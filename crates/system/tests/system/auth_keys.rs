use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use keel_sdk::chrono::Duration;
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{AuditEvent, AuditSink};
use keel_system::auth::{
    LOGIN_KEY_MAX_TTL, LOGIN_KEY_PREFIX, LOGIN_PATH, MODULE, REASON_REPLACED, TOKEN_PREFIX,
};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const JSON: (&str, &str) = ("content-type", "application/json");
const REDIRECT: &str = "/f/note.list";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).boot().await;
    let http = harness.http().await;
    (harness, http)
}

async fn issue(harness: &Harness, who: Who, params: Value) -> (String, String) {
    let reply = harness.call(who, "auth.key.issue", params).await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.key.issue by {who} failed: {:?}", reply.error()));
    let key = data["key"].as_str().expect("the key").to_owned();
    let url = data["url"].as_str().expect("the url").to_owned();
    (key, url)
}

async fn redeem(http: &TestHttp, key: &str, extra: &[(&str, &str)]) -> Answer {
    let mut headers = vec![JSON];
    headers.extend_from_slice(extra);
    http.request(
        "POST",
        "/api/v1/call/auth.key.redeem",
        &headers,
        Some(&json!({ "key": key }).to_string()),
    )
    .await
}

fn error_of(answer: &Answer, status: u16, code: &str, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["error"]["code"], json!(code), "{}", answer.body);
    assert_eq!(body["error"]["key"], json!(key), "{}", answer.body);
}

async fn post(http: &TestHttp, who: Who, params: Value) -> Answer {
    http.post(who, "/api/v1/call/auth.key.issue", &params).await
}

async fn create_principal(harness: &Harness, login: &str, level: Level, account: Id) -> Id {
    let reply = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": login, "level": level, "account": account }),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("keel.principal.create {login}: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

#[tokio::test]
async fn a_key_issued_for_a1_is_redeemed_once_and_refused_when_spent_expired_or_made_up() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let reseller = harness.principal(Who::Reseller).id;
    let (key, url) = issue(
        &harness,
        Who::Reseller,
        json!({ "principal": a1.id, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    assert!(key.starts_with(LOGIN_KEY_PREFIX), "{key}");
    assert_eq!(key.len(), LOGIN_KEY_PREFIX.len() + 64);
    assert_eq!(url, format!("{LOGIN_PATH}?key={key}"));
    let stored = harness
        .database()
        .rows("SELECT key_hash || ' ' || principal_id || ' ' || issued_by || ' ' || redirect || ' ' || (used_at IS NULL)::text FROM auth_login_key")
        .await;
    assert_eq!(stored.len(), 1);
    assert!(!stored[0].contains(&key), "{}", stored[0]);
    assert!(stored[0].starts_with(&format!("{} ", sha256(&key))));
    assert!(stored[0].ends_with(&format!("{} {reseller} {REDIRECT} true", a1.id)));

    let redeemed = redeem(&http, &key, &[]).await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);
    assert_eq!(
        redeemed.json()["data"],
        json!({
            "redirect": REDIRECT,
            "principal": { "id": a1.id, "login": "a1", "level": "user", "account": a1.account_id, "actor": null },
            "issuedBy": reseller
        })
    );
    let cookie = redeemed
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie: {:?}", redeemed.headers));
    assert!(cookie.contains(TOKEN_PREFIX), "{cookie}");
    let booted = http
        .request("GET", "/api/v1/boot", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["login"], json!("a1"));
    let used = harness
        .database()
        .rows("SELECT (used_at IS NOT NULL)::text FROM auth_login_key")
        .await;
    assert_eq!(used, ["true"]);

    let spent = redeem(&http, &key, &[]).await;
    error_of(&spent, 401, "unauthenticated", "auth.error.key");
    assert_eq!(spent.set_cookie(), None);

    let (short, _) = issue(
        &harness,
        Who::Reseller,
        json!({ "principal": a1.id, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    harness.clock().advance(Duration::seconds(61));
    let expired = redeem(&http, &short, &[]).await;
    error_of(&expired, 401, "unauthenticated", "auth.error.key");
    let unused = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_login_key WHERE used_at IS NULL")
        .await;
    assert_eq!(unused, ["1"]);

    let made_up = format!("{LOGIN_KEY_PREFIX}{}", "0".repeat(64));
    let unknown = redeem(&http, &made_up, &[]).await;
    error_of(&unknown, 401, "unauthenticated", "auth.error.key");
    let sessions = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_session")
        .await;
    assert_eq!(sessions, ["1"]);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn two_concurrent_redeems_of_one_key_answer_one_200_and_one_401() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).id;
    let (key, _) = issue(
        &harness,
        Who::Admin,
        json!({ "principal": a1, "ttl": 300, "redirect": "/" }),
    )
    .await;
    let (first, second) = tokio::join!(redeem(&http, &key, &[]), redeem(&http, &key, &[]));
    let mut statuses = [first.status, second.status];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 401], "{} / {}", first.body, second.body);
    assert_eq!(
        [first.set_cookie().is_some(), second.set_cookie().is_some()]
            .iter()
            .filter(|set| **set)
            .count(),
        1
    );
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM auth_session")
            .await,
        ["1"]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn issue_checks_the_ttl_the_redirect_the_scope_and_the_ladder_of_the_target() {
    let (harness, http) = boot().await;
    let accounts = harness.fixtures().accounts.clone();
    let a1 = harness.principal(Who::A1).id;
    let unrelated = harness.principal(Who::Unrelated).id;
    let admin = harness.principal(Who::Admin).id;
    let second_reseller =
        create_principal(&harness, "reseller2", Level::Reseller, accounts.reseller).await;
    let second_admin = create_principal(&harness, "admin2", Level::Admin, accounts.root).await;

    let long = post(
        &http,
        Who::Reseller,
        json!({ "principal": a1, "ttl": LOGIN_KEY_MAX_TTL + 1, "redirect": REDIRECT }),
    )
    .await;
    assert_eq!(long.status, 422, "{}", long.body);
    assert_eq!(long.json()["error"]["fields"][0]["path"], json!("ttl"));
    let zero = post(
        &http,
        Who::Reseller,
        json!({ "principal": a1, "ttl": 0, "redirect": REDIRECT }),
    )
    .await;
    assert_eq!(zero.status, 422, "{}", zero.body);
    assert_eq!(zero.json()["error"]["fields"][0]["path"], json!("ttl"));

    for redirect in [
        "https://evil.example",
        "//evil.example",
        "/\\evil.example",
        "evil",
    ] {
        let foreign = post(
            &http,
            Who::Reseller,
            json!({ "principal": a1, "ttl": 60, "redirect": redirect }),
        )
        .await;
        error_of(&foreign, 422, "validation", "keel.error.validation");
        assert_eq!(
            foreign.json()["error"]["fields"][0],
            json!({ "path": "redirect", "code": "auth.error.redirect", "key": "auth.error.redirect" }),
            "{redirect}"
        );
    }

    for target in [unrelated, admin] {
        let outside = post(
            &http,
            Who::Reseller,
            json!({ "principal": target, "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
        assert_eq!(outside.status, 404, "{target}: {}", outside.body);
        assert_eq!(outside.json()["error"]["code"], json!("not_found"));
    }
    let same_level = post(
        &http,
        Who::Reseller,
        json!({ "principal": second_reseller, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    error_of(&same_level, 403, "forbidden", "auth.error.not_below");
    let peer = post(
        &http,
        Who::Admin,
        json!({ "principal": second_admin, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    error_of(&peer, 403, "forbidden", "auth.error.not_below");
    let by_user = post(
        &http,
        Who::A1,
        json!({ "principal": a1, "ttl": 60, "redirect": REDIRECT }),
    )
    .await;
    error_of(&by_user, 403, "forbidden", "keel.error.level");

    let issued = post(
        &http,
        Who::Reseller,
        json!({ "principal": a1, "ttl": LOGIN_KEY_MAX_TTL, "redirect": REDIRECT }),
    )
    .await;
    assert_eq!(issued.status, 200, "{}", issued.body);
    let count = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_login_key")
        .await;
    assert_eq!(count, ["1"]);
    let leaked = harness
        .database()
        .rows(r"SELECT count(*)::text FROM auth_login_key k WHERE k::text LIKE '%kok\_%'")
        .await;
    assert_eq!(leaked, ["0"]);
    let socket = harness.socket().await;
    let by_owner = socket
        .call(
            "auth.key.issue",
            json!({ "principal": a1, "ttl": 60, "redirect": REDIRECT }),
        )
        .await;
    let by_owner = serde_json::to_value(&by_owner).expect("an envelope");
    assert_eq!(by_owner["ok"], json!(true), "{by_owner}");
    assert!(
        by_owner["data"]["key"]
            .as_str()
            .is_some_and(|key| key.starts_with(LOGIN_KEY_PREFIX)),
        "{by_owner}"
    );
    let redeem_over_socket = socket
        .call("auth.key.redeem", json!({ "key": by_owner["data"]["key"] }))
        .await;
    let redeem_over_socket = serde_json::to_value(&redeem_over_socket).expect("an envelope");
    assert_eq!(
        redeem_over_socket["error"]["key"],
        json!("auth.error.http_only"),
        "{redeem_over_socket}"
    );
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AuditEvent>>,
}

impl AuditSink for Recorder {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

static RECORDER: std::sync::LazyLock<Arc<Recorder>> =
    std::sync::LazyLock::new(|| Arc::new(Recorder::default()));

static AUDIT: ModuleDef = ModuleDef {
    id: "auditprobe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: audit_init,
};

fn audit_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn AuditSink>(RECORDER.clone());
    Ok(())
}

#[tokio::test]
async fn redeem_is_refused_cross_site_replaces_the_session_it_arrived_with_and_masks_the_key() {
    let harness = Harness::new().module(&MODULE).module(&AUDIT).boot().await;
    let http = harness.http().await;
    let a1 = harness.principal(Who::A1).id;
    let (key, _) = issue(
        &harness,
        Who::Admin,
        json!({ "principal": a1, "ttl": 120, "redirect": REDIRECT }),
    )
    .await;
    let refused = redeem(&http, &key, &[("origin", "https://evil.example")]).await;
    error_of(&refused, 403, "forbidden", "keel.error.cross_site");
    assert_eq!(refused.set_cookie(), None);
    let fetched = redeem(&http, &key, &[("sec-fetch-site", "cross-site")]).await;
    assert_eq!(fetched.status, 403, "{}", fetched.body);
    let unused = harness
        .database()
        .rows("SELECT (used_at IS NULL)::text FROM auth_login_key")
        .await;
    assert_eq!(unused, ["true"]);

    let first = redeem(&http, &key, &[]).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let first_cookie = first.session_cookie().expect("the first cookie");
    let (second_key, _) = issue(
        &harness,
        Who::Admin,
        json!({ "principal": a1, "ttl": 120, "redirect": "/" }),
    )
    .await;
    let second = redeem(&http, &second_key, &[("cookie", &first_cookie)]).await;
    assert_eq!(second.status, 200, "{}", second.body);
    let second_cookie = second.session_cookie().expect("the second cookie");
    assert_ne!(first_cookie, second_cookie);
    let reasons = harness
        .database()
        .rows("SELECT reason FROM auth_session WHERE revoked_at IS NOT NULL")
        .await;
    assert_eq!(reasons, [REASON_REPLACED]);
    let stale = http
        .request("GET", "/api/v1/boot", &[("cookie", &first_cookie)], None)
        .await;
    assert_eq!(stale.status, 401, "{}", stale.body);

    let events: Vec<AuditEvent> = RECORDER
        .events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|event| event.function == "auth.key.redeem")
        .cloned()
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_eq!(event.params["key"], json!("***"));
        assert!(event.outcome.is_committed(), "{event:?}");
    }
    let recorded = serde_json::to_string(&events).expect("events serialise");
    assert!(!recorded.contains(&key));
    assert!(!recorded.contains(&second_key));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_key_descriptors_match_their_snapshots_and_the_key_is_secret_and_shown_once() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let names = harness.snapshot_descriptors(Level::Guest, snapshots_dir());
    assert_eq!(names, ["auth.key.redeem", "auth.login", "auth.totp.verify"]);
    let names = harness.snapshot_descriptors(Level::Reseller, snapshots_dir());
    assert!(names.contains(&"auth.key.issue".to_owned()), "{names:?}");
    let registry = harness.registry();
    let issue = registry
        .function("auth.key.issue")
        .expect("auth.key.issue is registered")
        .descriptor();
    let params = serde_json::to_value(&issue.params).expect("a schema");
    assert_eq!(params["properties"]["ttl"]["maximum"], json!(300));
    assert_eq!(params["properties"]["ttl"]["minimum"], json!(1));
    let result = serde_json::to_value(&issue.result).expect("a schema");
    assert_eq!(result["properties"]["key"]["x-keel"]["once"], json!(true));
    assert_eq!(result["properties"]["url"]["x-keel"]["once"], json!(true));
    let redeem = registry
        .function("auth.key.redeem")
        .expect("auth.key.redeem is registered")
        .descriptor();
    let params = serde_json::to_value(&redeem.params).expect("a schema");
    assert_eq!(params["properties"]["key"]["x-keel"]["secret"], json!(true));
    harness.finish().await;
}
