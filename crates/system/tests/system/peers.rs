use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, TransactionTrait};
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, SecretCipher};
use keel_sdk::types::{ErrorCode, InstanceInfo, PROTOCOL};
use keel_system::auth::api::token_hash;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::notify::api::{Problems, Raise, Severity};
use keel_system::peers::{
    CODE_PREFIX, CREATE, DELETE, ERROR_CODE, ERROR_INSTANCE_KEY, ERROR_LEVEL, ERROR_MASK,
    ERROR_NO_URL, ERROR_PAIRED, ERROR_PROTOCOL, ERROR_SELF, ERROR_STORED, ERROR_TOKEN,
    ERROR_UNREACHABLE, ERROR_URL, FEATURE, GET, JOB_ID, LIST, LOGIN_PREFIX, MODULE, PAIR,
    PAIR_CODE, PROBLEM_INSTANCE_KEY, PROBLEM_PROTOCOL, RENEW, ROLE, ROTATE, SOURCE, STATUS_ACTIVE,
};
use keel_testkit::{Answer, Harness, Reply, TestHttp, Who, tokio};

const OLDER: &str = "01J9A00000000000000000000A";
const NEWER: &str = "01J9B00000000000000000000B";
const PATIENCE: Duration = Duration::from_secs(30);
const PASSWORD: &str = "correct-horse-battery-staple";
const AGENT: &str = "keel-system-tests/1";

pub(crate) async fn installation() -> Harness {
    composed(Harness::new()).boot().await
}

pub(crate) fn composed(builder: keel_testkit::Builder) -> keel_testkit::Builder {
    builder
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::webhooks::MODULE)
        .module(&MODULE)
}

pub(crate) struct Two {
    pub(crate) a: Harness,
    pub(crate) b: Harness,
    pub(crate) a_http: TestHttp,
    pub(crate) b_http: TestHttp,
}

pub(crate) struct Linked {
    pub(crate) a_link: String,
    pub(crate) b_link: String,
}

impl Two {
    pub(crate) async fn start() -> Self {
        Self::of(installation().await, installation().await).await
    }

    pub(crate) async fn of(a: Harness, b: Harness) -> Self {
        let a_http = a.http().await;
        let b_http = b.http().await;
        Self {
            a,
            b,
            a_http,
            b_http,
        }
    }

    fn b_url(&self) -> String {
        self.b_http.url("")
    }

    pub(crate) async fn pair(&self, b_grants: Value, a_grants: Value) -> Linked {
        let code = code(&self.b, b_grants).await;
        let mut params = a_grants;
        params["url"] = json!(self.b_url());
        params["code"] = json!(code);
        let created = self.a.call(Who::Admin, CREATE, params).await;
        assert_eq!(created.status(), 200, "{created:?}");
        let a_link = data(&created)["id"].as_str().unwrap().to_owned();
        let b_link = only_link(&self.b).await;
        Linked { a_link, b_link }
    }

    pub(crate) async fn finish(self) {
        self.a_http.stop().await.unwrap();
        self.b_http.stop().await.unwrap();
        self.a.finish().await;
        self.b.finish().await;
    }
}

pub(crate) fn data(reply: &Reply) -> &Value {
    match reply.data() {
        Some(data) => data,
        None => panic!("{reply:?}"),
    }
}

pub(crate) async fn code(harness: &Harness, grants: Value) -> String {
    let issued = harness.call(Who::Admin, PAIR_CODE, grants).await;
    assert_eq!(issued.status(), 200, "{issued:?}");
    data(&issued)["code"].as_str().unwrap().to_owned()
}

async fn only_link(harness: &Harness) -> String {
    let ids = harness.database().rows("SELECT id FROM peer_product").await;
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids[0].clone()
}

pub(crate) async fn outbound(harness: &Harness, link: &str) -> String {
    let sealed = harness
        .database()
        .rows(&format!(
            "SELECT outbound_token_enc FROM peer_product WHERE id = '{link}'"
        ))
        .await;
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel seals with the data key");
    String::from_utf8(cipher.open(&sealed[0]).expect("the data key opens it")).unwrap()
}

pub(crate) async fn call_with(http: &TestHttp, token: &str, func: &str, params: Value) -> Answer {
    let bearer = format!("Bearer {token}");
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &[
            ("authorization", bearer.as_str()),
            ("content-type", "application/json"),
        ],
        Some(&params.to_string()),
    )
    .await
}

pub(crate) fn refused(answer: &Answer, status: u16, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    assert_eq!(answer.json()["error"]["key"], json!(key), "{}", answer.body);
}

fn field_error(reply: &Reply, code: ErrorCode, path: &str, key: &str) {
    let error = reply.error().unwrap_or_else(|| panic!("{reply:?}"));
    assert_eq!(error.code, code, "{reply:?}");
    let field = error
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("no field {path} in {reply:?}"));
    assert_eq!(field.key.as_deref(), Some(key), "{reply:?}");
}

fn error_key(reply: &Reply, code: ErrorCode, key: &str) {
    let error = reply.error().unwrap_or_else(|| panic!("{reply:?}"));
    assert_eq!((error.code, error.key.as_str()), (code, key), "{reply:?}");
}

pub(crate) async fn run_check(harness: &Harness) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !scheduler.is_leader() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduler did not lead"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = scheduler.run_now(JOB_ID).await.expect("the job starts");
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the run ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok);
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key || ' ' || subject FROM notify_problem WHERE cleared_at IS NULL ORDER BY key, subject")
        .await
}

async fn until_open_problems(harness: &Harness, expected: &[String]) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let open = open_problems(harness).await;
        if open == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{open:?} instead of {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn link_state(harness: &Harness, link: &str) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT inbound_token_hash || ' ' || outbound_token_enc FROM peer_product WHERE id = '{link}'"
        ))
        .await
}

pub(crate) async fn link_principal(harness: &Harness, link: &str) -> String {
    let ids = harness
        .database()
        .rows(&format!(
            "SELECT principal_id FROM peer_product WHERE id = '{link}'"
        ))
        .await;
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids[0].clone()
}

pub(crate) async fn mint(http: &TestHttp, peer_token: &str) -> String {
    let created = call_with(
        http,
        peer_token,
        "auth.token.create",
        json!({ "name": "derived" }),
    )
    .await;
    assert_eq!(created.status, 200, "{}", created.body);
    let minted = created.json()["data"]["token"].as_str().unwrap().to_owned();
    assert!(minted.starts_with("kat_"), "{minted}");
    minted
}

async fn login(http: &TestHttp, login: &str) -> Answer {
    http.request(
        "POST",
        "/api/v1/call/auth.login",
        &[("content-type", "application/json"), ("user-agent", AGENT)],
        Some(&json!({ "login": login, "password": PASSWORD }).to_string()),
    )
    .await
}

async fn boot_with(http: &TestHttp, cookie: &str) -> Answer {
    http.request(
        "GET",
        "/api/v1/boot",
        &[("cookie", cookie), ("user-agent", AGENT)],
        None,
    )
    .await
}

async fn until_waiting_on_a_lock(harness: &Harness) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let waiting = harness
            .database()
            .rows("SELECT count(*)::text FROM pg_stat_activity WHERE datname = current_database() AND wait_event = 'transactionid'")
            .await;
        if waiting != ["0"] {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no call waits on the held row: {waiting:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn two_installations_pair_by_code_and_call_each_other_within_scope() {
    let two = Two::start().await;
    let b_grants = json!({ "functions": ["keel.account.list", "peer.list"], "level": "admin", "onBehalf": false });
    let a_grants =
        json!({ "functions": ["keel.principal.list"], "level": "admin", "onBehalf": false });

    let code = code(
        &two.b,
        json!({ "scopes": ["keel.account.list", "peer.list"], "level": "admin", "ttl": 600 }),
    )
    .await;
    let rest = code.strip_prefix(CODE_PREFIX).expect("PAIR-XXXX-XXXX");
    assert_eq!(rest.len(), 9, "{code}");
    assert_eq!(&rest[4..5], "-", "{code}");

    let created = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": code, "scopes": ["keel.principal.list"], "level": "admin" }),
        )
        .await;
    assert_eq!(created.status(), 200, "{created:?}");
    let paired = data(&created);
    assert_eq!(paired["instanceId"], json!(two.b.instance().to_string()));
    assert_eq!(
        paired["publicKey"],
        json!(two.b.instance_key().public_key())
    );
    assert_eq!(paired["url"], json!(two.b_url()));
    assert_eq!(paired["protocol"], json!(PROTOCOL));
    assert_eq!(paired["status"], json!(STATUS_ACTIVE));
    assert_eq!(paired["scopes"], a_grants);
    assert_eq!(paired["granted"], b_grants);
    let a_link = paired["id"].as_str().unwrap().to_owned();

    let on_a = two.a.call(Who::Admin, GET, json!({ "id": a_link })).await;
    assert_eq!(
        data(&on_a)["instanceId"],
        json!(two.b.instance().to_string())
    );
    let b_link = only_link(&two.b).await;
    let on_b = two.b.call(Who::Admin, GET, json!({ "id": b_link })).await;
    let on_b = data(&on_b);
    assert_eq!(on_b["instanceId"], json!(two.a.instance().to_string()));
    assert_eq!(on_b["publicKey"], json!(two.a.instance_key().public_key()));
    assert_eq!(on_b["url"], Value::Null);
    assert_eq!(on_b["scopes"], b_grants);
    assert!(on_b["products"].as_array().is_some(), "{on_b}");
    assert_eq!(
        two.b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["0"]
    );

    for (harness, link) in [(&two.a, &a_link), (&two.b, &b_link)] {
        let principal = harness
            .database()
            .rows(&format!(
                "SELECT p.login || ' ' || p.level || ' ' || p.account_id || ' ' || p.role FROM keel_principal p JOIN peer_product l ON l.principal_id = p.id WHERE l.id = '{link}'"
            ))
            .await;
        assert_eq!(
            principal,
            [format!(
                "{LOGIN_PREFIX}{} service {} {ROLE}",
                link.to_lowercase(),
                Id::ROOT_ACCOUNT
            )]
        );
    }

    let a_token = outbound(&two.a, &a_link).await;
    let b_token = outbound(&two.b, &b_link).await;
    assert!(a_token.starts_with("kpt_") && b_token.starts_with("kpt_"));
    assert_ne!(a_token, b_token);
    assert_eq!(
        two.b
            .database()
            .rows(&format!(
                "SELECT inbound_token_hash FROM peer_product WHERE id = '{b_link}'"
            ))
            .await,
        [token_hash(&a_token)]
    );
    for (harness, token) in [
        (&two.a, &a_token),
        (&two.a, &b_token),
        (&two.b, &a_token),
        (&two.b, &b_token),
    ] {
        let stored = harness
            .database()
            .rows("SELECT row_to_json(l)::text FROM peer_product l")
            .await;
        assert!(!stored[0].contains(token.as_str()), "{stored:?}");
    }

    let listed = call_with(&two.b_http, &a_token, "keel.account.list", json!({})).await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let principal = &listed.json()["meta"]["principal"];
    assert_eq!(principal["level"], json!("admin"));
    assert_eq!(
        principal["login"],
        json!(format!("{LOGIN_PREFIX}{}", b_link.to_lowercase()))
    );
    assert_eq!(principal["actor"], Value::Null);
    let peers = call_with(&two.b_http, &a_token, LIST, json!({})).await;
    assert_eq!(peers.status, 200, "{}", peers.body);
    assert_eq!(
        peers.json()["data"]["data"][0]["instanceId"],
        json!(two.a.instance().to_string())
    );

    let back = call_with(&two.a_http, &b_token, "keel.principal.list", json!({})).await;
    assert_eq!(back.status, 200, "{}", back.body);
    assert_eq!(
        back.json()["meta"]["principal"]["login"],
        json!(format!("{LOGIN_PREFIX}{}", a_link.to_lowercase()))
    );
    let seen = two
        .b
        .database()
        .rows(&format!(
            "SELECT (last_seen_at IS NOT NULL)::text FROM peer_product WHERE id = '{b_link}'"
        ))
        .await;
    assert_eq!(seen, ["true"]);

    two.finish().await;
}

#[tokio::test]
async fn a_call_outside_the_token_functions_answers_forbidden() {
    let two = Two::start().await;
    let linked = two
        .pair(
            json!({ "scopes": ["keel.account.list", "peer.list"], "level": "user" }),
            json!({ "scopes": ["keel.principal.list"], "level": "admin" }),
        )
        .await;
    let a_token = outbound(&two.a, &linked.a_link).await;
    let b_token = outbound(&two.b, &linked.b_link).await;

    let within = call_with(&two.b_http, &a_token, "keel.account.list", json!({})).await;
    assert_eq!(within.status, 200, "{}", within.body);
    assert_eq!(within.json()["meta"]["principal"]["level"], json!("user"));

    let outside = call_with(&two.b_http, &a_token, "keel.principal.list", json!({})).await;
    refused(&outside, 403, "keel.error.token_scope");
    assert_eq!(
        outside.json()["error"]["params"]["function"],
        json!("keel.principal.list")
    );
    let above = call_with(&two.b_http, &a_token, LIST, json!({})).await;
    refused(&above, 403, "keel.error.level");

    let back_outside = call_with(&two.a_http, &b_token, "keel.account.list", json!({})).await;
    refused(&back_outside, 403, "keel.error.token_scope");
    let pairing = call_with(&two.a_http, &b_token, PAIR_CODE, json!({ "scopes": [] })).await;
    refused(&pairing, 403, "keel.error.token_scope");

    let unknown = call_with(
        &two.b_http,
        "kpt_0000000000000000000000000000000000000000000000000000000000000000",
        "keel.account.list",
        json!({}),
    )
    .await;
    refused(&unknown, 401, "keel.error.unauthenticated");

    two.finish().await;
}

#[tokio::test]
async fn a_used_or_expired_code_is_refused() {
    let two = Two::start().await;
    let first = code(&two.b, json!({ "scopes": ["keel.account.list"] })).await;
    let created = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": first }),
        )
        .await;
    assert_eq!(created.status(), 200, "{created:?}");
    let a_link = data(&created)["id"].as_str().unwrap().to_owned();
    let b_link = only_link(&two.b).await;
    for (harness, link) in [(&two.a, &a_link), (&two.b, &b_link)] {
        let deleted = harness
            .call(Who::Admin, DELETE, json!({ "id": link }))
            .await;
        assert_eq!(data(&deleted)["deleted"], json!(true));
    }

    let used = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": first }),
        )
        .await;
    field_error(&used, ErrorCode::Peer, "code", ERROR_CODE);
    assert_eq!(used.status(), 502, "{used:?}");

    let lowered = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": first.to_lowercase() }),
        )
        .await;
    field_error(&lowered, ErrorCode::Peer, "code", ERROR_CODE);
    let unknown = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": "PAIR-0000-0000" }),
        )
        .await;
    field_error(&unknown, ErrorCode::Peer, "code", ERROR_CODE);

    let direct = two
        .b_http
        .post(
            Who::Guest,
            &format!("/api/v1/call/{PAIR}"),
            &json!({
                "code": first,
                "instance": {
                    "instanceId": two.a.instance().to_string(), "keel": "0.3.0", "protocol": PROTOCOL,
                    "products": [], "publicKey": two.a.instance_key().public_key()
                },
                "inboundToken": format!("kpt_{}", "a".repeat(64)),
                "inboundScopes": { "functions": [], "level": "service" }
            }),
        )
        .await;
    assert_eq!(direct.status, 422, "{}", direct.body);
    assert_eq!(direct.json()["error"]["fields"][0]["path"], json!("code"));
    assert_eq!(
        direct.json()["error"]["fields"][0]["key"],
        json!(ERROR_CODE)
    );

    let short = code(&two.b, json!({ "scopes": [], "ttl": 60 })).await;
    two.b
        .clock()
        .advance(keel_sdk::chrono::Duration::seconds(61));
    let expired = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": short }),
        )
        .await;
    field_error(&expired, ErrorCode::Peer, "code", ERROR_CODE);
    assert!(
        two.b
            .database()
            .rows("SELECT id FROM peer_product")
            .await
            .is_empty()
    );
    assert!(
        two.a
            .database()
            .rows("SELECT id FROM peer_product")
            .await
            .is_empty()
    );
    assert_eq!(
        two.b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["1"]
    );
    run_check(&two.b).await;
    assert_eq!(
        two.b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["0"]
    );

    two.finish().await;
}

#[tokio::test]
async fn a_peer_presenting_another_instance_key_is_refused() {
    let two = Two::start().await;
    let linked = two
        .pair(
            json!({ "scopes": ["keel.account.list"], "level": "admin" }),
            json!({ "scopes": ["keel.account.list"], "level": "admin" }),
        )
        .await;
    let a_token = outbound(&two.a, &linked.a_link).await;

    let impostor_of_b = composed(Harness::new().instance(two.b.instance()))
        .boot()
        .await;
    let impostor_http = impostor_of_b.http().await;
    assert_ne!(
        impostor_of_b.instance_key().public_key(),
        two.b.instance_key().public_key()
    );

    let fresh = code(&impostor_of_b, json!({ "scopes": [] })).await;
    let towards_impostor = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": impostor_http.url(""), "code": fresh }),
        )
        .await;
    error_key(&towards_impostor, ErrorCode::Forbidden, ERROR_INSTANCE_KEY);
    assert_eq!(
        impostor_of_b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["1"]
    );

    two.a
        .db()
        .execute_unprepared(&format!(
            "UPDATE peer_product SET url = '{}', keel_version = 'pinned' WHERE id = '{}'",
            impostor_http.url(""),
            linked.a_link
        ))
        .await
        .unwrap();
    run_check(&two.a).await;
    assert_eq!(
        open_problems(&two.a).await,
        [format!("{PROBLEM_INSTANCE_KEY} {}", linked.a_link)]
    );
    assert_eq!(
        two.a
            .database()
            .rows(&format!(
                "SELECT keel_version || ' ' || public_key FROM peer_product WHERE id = '{}'",
                linked.a_link
            ))
            .await,
        [format!("pinned {}", two.b.instance_key().public_key())]
    );
    let rotated = two
        .a
        .call(Who::Admin, ROTATE, json!({ "id": linked.a_link }))
        .await;
    error_key(&rotated, ErrorCode::Forbidden, ERROR_INSTANCE_KEY);
    let still = call_with(&two.b_http, &a_token, "keel.account.list", json!({})).await;
    assert_eq!(still.status, 200, "{}", still.body);

    two.a
        .db()
        .execute_unprepared(&format!(
            "UPDATE peer_product SET url = '{}' WHERE id = '{}'",
            two.b_url(),
            linked.a_link
        ))
        .await
        .unwrap();
    run_check(&two.a).await;
    assert!(open_problems(&two.a).await.is_empty());
    assert_ne!(
        two.a
            .database()
            .rows(&format!(
                "SELECT keel_version FROM peer_product WHERE id = '{}'",
                linked.a_link
            ))
            .await,
        ["pinned"]
    );

    let impostor_of_a = composed(Harness::new().instance(two.a.instance()))
        .boot()
        .await;
    let second = code(&two.b, json!({ "scopes": [] })).await;
    let towards_b = impostor_of_a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": second }),
        )
        .await;
    error_key(&towards_b, ErrorCode::Peer, ERROR_INSTANCE_KEY);
    assert_eq!(towards_b.status(), 502);
    assert_eq!(
        two.b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["1"]
    );
    assert_eq!(
        two.b
            .database()
            .rows("SELECT public_key FROM peer_product")
            .await,
        [two.a.instance_key().public_key()]
    );

    impostor_http.stop().await.unwrap();
    impostor_of_b.finish().await;
    impostor_of_a.finish().await;
    two.finish().await;
}

#[tokio::test]
async fn rotate_retires_the_old_token_and_delete_answers_401() {
    let two = Two::start().await;
    let linked = two
        .pair(
            json!({ "scopes": ["keel.account.list"], "level": "admin" }),
            json!({ "scopes": ["keel.account.list"], "level": "admin" }),
        )
        .await;
    let old_a = outbound(&two.a, &linked.a_link).await;
    let old_b = outbound(&two.b, &linked.b_link).await;
    assert_eq!(
        call_with(&two.b_http, &old_a, "keel.account.list", json!({}))
            .await
            .status,
        200
    );

    let rotated = two
        .a
        .call(Who::Admin, ROTATE, json!({ "id": linked.a_link }))
        .await;
    assert_eq!(data(&rotated)["rotated"], json!(true));

    let retired_on_b = call_with(&two.b_http, &old_a, "keel.account.list", json!({})).await;
    refused(&retired_on_b, 401, "keel.error.unauthenticated");
    let retired_on_a = call_with(&two.a_http, &old_b, "keel.account.list", json!({})).await;
    refused(&retired_on_a, 401, "keel.error.unauthenticated");
    let new_a = outbound(&two.a, &linked.a_link).await;
    let new_b = outbound(&two.b, &linked.b_link).await;
    assert_ne!(new_a, old_a);
    assert_ne!(new_b, old_b);

    let renew = format!("/api/v1/call/{RENEW}");
    let kept = link_state(&two.b, &linked.b_link).await;
    let offered = format!("kpt_{}", "c".repeat(64));
    for token in [
        old_a.as_str(),
        "kpt_0000000000000000000000000000000000000000000000000000000000000000",
        "garbage",
    ] {
        let renewed = two
            .b_http
            .post(
                Who::Guest,
                &renew,
                &json!({ "token": token, "inboundToken": offered }),
            )
            .await;
        refused(&renewed, 401, ERROR_TOKEN);
    }
    let api_token = two
        .b_http
        .post(
            Who::Guest,
            &renew,
            &json!({ "token": new_a, "inboundToken": format!("kat_{}", "c".repeat(64)) }),
        )
        .await;
    assert_eq!(api_token.status, 422, "{}", api_token.body);
    assert_eq!(
        api_token.json()["error"]["fields"][0]["path"],
        json!("inboundToken")
    );
    assert_eq!(
        api_token.json()["error"]["fields"][0]["key"],
        json!(ERROR_TOKEN)
    );
    assert_eq!(link_state(&two.b, &linked.b_link).await, kept);
    assert_eq!(
        call_with(&two.b_http, &new_a, "keel.account.list", json!({}))
            .await
            .status,
        200
    );
    assert_eq!(
        call_with(&two.a_http, &new_b, "keel.account.list", json!({}))
            .await
            .status,
        200
    );

    let from_b = two
        .b
        .call(Who::Admin, ROTATE, json!({ "id": linked.b_link }))
        .await;
    error_key(&from_b, ErrorCode::Conflict, ERROR_NO_URL);

    let deleted = two
        .b
        .call(Who::Admin, DELETE, json!({ "id": linked.b_link }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(true));
    let revoked = call_with(&two.b_http, &new_a, "keel.account.list", json!({})).await;
    refused(&revoked, 401, "keel.error.unauthenticated");
    let gone = two
        .b
        .call(Who::Admin, GET, json!({ "id": linked.b_link }))
        .await;
    error_key(&gone, ErrorCode::NotFound, "keel.error.not_found");
    let again = two
        .b
        .call(Who::Admin, DELETE, json!({ "id": linked.b_link }))
        .await;
    error_key(&again, ErrorCode::NotFound, "keel.error.not_found");

    two.finish().await;
}

#[tokio::test]
async fn credentials_minted_under_a_peer_token_die_with_its_rotation_and_its_deletion() {
    let two = Two::start().await;
    let grants = json!({ "scopes": ["keel.account.list", "keel.principal.get", "auth.*"], "level": "admin" });
    let linked = two.pair(grants.clone(), grants).await;
    let a_token = outbound(&two.a, &linked.a_link).await;
    let b_token = outbound(&two.b, &linked.b_link).await;

    let minted = [
        (&two.b_http, mint(&two.b_http, &a_token).await),
        (&two.a_http, mint(&two.a_http, &b_token).await),
    ];
    for (http, token) in &minted {
        let derived = call_with(http, token, "keel.account.list", json!({})).await;
        refused(&derived, 403, "keel.error.level");
    }
    let rotated = two
        .a
        .call(Who::Admin, ROTATE, json!({ "id": linked.a_link }))
        .await;
    assert_eq!(data(&rotated)["rotated"], json!(true));
    for (http, token) in &minted {
        let retired = call_with(http, token, "keel.account.list", json!({})).await;
        refused(&retired, 401, "keel.error.unauthenticated");
    }

    let a_token = outbound(&two.a, &linked.a_link).await;
    let derived = mint(&two.b_http, &a_token).await;
    let principal = link_principal(&two.b, &linked.b_link).await;
    let set = call_with(
        &two.b_http,
        &a_token,
        "auth.password.set",
        json!({ "principal": principal, "password": PASSWORD }),
    )
    .await;
    assert_eq!(set.status, 200, "{}", set.body);
    let name = format!("{LOGIN_PREFIX}{}", linked.b_link.to_lowercase());
    let logged_in = login(&two.b_http, &name).await;
    assert_eq!(logged_in.status, 200, "{}", logged_in.body);
    let cookie = logged_in.session_cookie().expect("a session cookie");
    let booted = boot_with(&two.b_http, &cookie).await;
    assert_eq!(booted.status, 200, "{}", booted.body);

    let deleted = two
        .b
        .call(Who::Admin, DELETE, json!({ "id": linked.b_link }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(true));
    let token_gone = call_with(&two.b_http, &derived, "keel.account.list", json!({})).await;
    refused(&token_gone, 401, "keel.error.unauthenticated");
    refused(
        &boot_with(&two.b_http, &cookie).await,
        401,
        "keel.error.unauthenticated",
    );
    refused(
        &login(&two.b_http, &name).await,
        401,
        "auth.error.credentials",
    );
    assert_eq!(
        two.b
            .database()
            .rows(&format!(
                "SELECT (SELECT count(*) FROM auth_password WHERE principal_id = p.id)::text || ' ' || p.login FROM keel_principal p WHERE p.id = '{principal}'"
            ))
            .await,
        [format!("0 {name}")]
    );

    two.finish().await;
}

#[tokio::test]
async fn deleting_a_link_clears_its_problems_and_the_check_clears_those_of_links_gone() {
    let two = Two::start().await;
    let linked = two
        .pair(json!({ "scopes": [] }), json!({ "scopes": [] }))
        .await;
    let impostor = composed(Harness::new().instance(two.b.instance()))
        .boot()
        .await;
    let impostor_http = impostor.http().await;
    two.a
        .db()
        .execute_unprepared(&format!(
            "UPDATE peer_product SET url = '{}' WHERE id = '{}'",
            impostor_http.url(""),
            linked.a_link
        ))
        .await
        .unwrap();
    run_check(&two.a).await;
    assert_eq!(
        open_problems(&two.a).await,
        [format!("{PROBLEM_INSTANCE_KEY} {}", linked.a_link)]
    );

    let caller = two
        .a
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    let gone = "01J9C00000000000000000000C";
    for (subject, source) in [(gone, SOURCE), ("elsewhere", "peer.other")] {
        Problems::raise(
            caller.as_ref(),
            Raise::new(PROBLEM_PROTOCOL, subject, Severity::Warning).source(source),
        )
        .await
        .expect("the problem is raised");
    }

    let deleted = two
        .a
        .call(Who::Admin, DELETE, json!({ "id": linked.a_link }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(true));
    until_open_problems(
        &two.a,
        &[
            format!("{PROBLEM_PROTOCOL} {gone}"),
            format!("{PROBLEM_PROTOCOL} elsewhere"),
        ],
    )
    .await;
    run_check(&two.a).await;
    assert_eq!(
        open_problems(&two.a).await,
        [format!("{PROBLEM_PROTOCOL} elsewhere")]
    );

    impostor_http.stop().await.unwrap();
    impostor.finish().await;
    two.finish().await;
}

#[tokio::test]
async fn a_pairing_racing_another_of_the_same_instance_answers_its_refusal() {
    let b = installation().await;
    let b_http = b.http().await;
    let path = format!("/api/v1/call/{PAIR}");
    let pinned = "ed25519:MCowBQYDK2VwAyEApinned";
    for (held_key, status, key) in [
        ("ed25519:MCowBQYDK2VwAyEAother", 403, ERROR_INSTANCE_KEY),
        (pinned, 409, ERROR_PAIRED),
    ] {
        let code = code(&b, json!({ "scopes": [] })).await;
        let held = b.db().begin().await.unwrap();
        held.execute_unprepared(&format!(
            "INSERT INTO peer_product (id, instance_id, products_json, keel_version, protocol, \
             public_key, inbound_token_hash, inbound_scopes_json, outbound_token_enc, status, \
             created_at) VALUES ('{NEWER}', '{OLDER}', '[]', '0.3.0', {PROTOCOL}, '{held_key}', \
             'held-hash', '{{\"functions\": [], \"level\": \"service\"}}', 'sealed', \
             '{STATUS_ACTIVE}', now())"
        ))
        .await
        .unwrap();
        let body = json!({
            "code": code,
            "instance": {
                "instanceId": OLDER, "keel": "0.3.0", "protocol": PROTOCOL,
                "products": [], "publicKey": pinned
            },
            "inboundToken": format!("kpt_{}", "d".repeat(64)),
            "inboundScopes": { "functions": [], "level": "service" }
        });
        let racing = b_http.post(Who::Guest, &path, &body);
        let release = async {
            until_waiting_on_a_lock(&b).await;
            held.commit().await.unwrap();
        };
        let (answer, ()) = tokio::join!(racing, release);
        refused(&answer, status, key);
        b.db()
            .execute_unprepared("DELETE FROM peer_product")
            .await
            .unwrap();
    }
    assert_eq!(
        b.database()
            .rows("SELECT (SELECT count(*) FROM peer_pair_code)::text || ' ' || (SELECT count(*) FROM keel_principal WHERE login LIKE 'peer-%')::text")
            .await,
        ["2 0"]
    );

    b_http.stop().await.unwrap();
    b.finish().await;
}

#[tokio::test]
async fn pairing_refuses_itself_a_paired_instance_an_unreachable_address_and_a_far_protocol() {
    let two = Two::start().await;
    let own = code(&two.a, json!({ "scopes": [] })).await;
    let itself = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.a_http.url(""), "code": own }),
        )
        .await;
    error_key(&itself, ErrorCode::Conflict, ERROR_SELF);

    let nowhere = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": "http://127.0.0.1:1", "code": "PAIR-0000-0000" }),
        )
        .await;
    error_key(&nowhere, ErrorCode::Peer, ERROR_UNREACHABLE);
    assert_eq!(nowhere.status(), 502);
    let malformed = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": "dns.example", "code": "PAIR-0000-0000" }),
        )
        .await;
    field_error(&malformed, ErrorCode::Validation, "url", ERROR_URL);

    two.pair(json!({ "scopes": [] }), json!({ "scopes": [] }))
        .await;
    let second = code(&two.b, json!({ "scopes": [] })).await;
    let twice = two
        .a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": two.b_url(), "code": second }),
        )
        .await;
    error_key(&twice, ErrorCode::Conflict, ERROR_PAIRED);

    let instance = |protocol: u32| {
        json!({
            "instanceId": two.a.instance().to_string(), "keel": "0.3.0", "protocol": protocol,
            "products": [], "publicKey": two.a.instance_key().public_key()
        })
    };
    let pair = |instance: Value| {
        json!({
            "code": second,
            "instance": instance,
            "inboundToken": format!("kpt_{}", "b".repeat(64)),
            "inboundScopes": { "functions": [], "level": "service" }
        })
    };
    let path = format!("/api/v1/call/{PAIR}");
    let paired = two
        .b_http
        .post(Who::Guest, &path, &pair(instance(PROTOCOL)))
        .await;
    refused(&paired, 409, ERROR_PAIRED);
    let far = two
        .b_http
        .post(Who::Guest, &path, &pair(instance(PROTOCOL + 2)))
        .await;
    refused(&far, 409, ERROR_PROTOCOL);
    let mut bad_token = pair(instance(PROTOCOL));
    bad_token["inboundToken"] = json!("kat_abc");
    let bad = two.b_http.post(Who::Guest, &path, &bad_token).await;
    assert_eq!(bad.status, 422, "{}", bad.body);
    assert_eq!(
        bad.json()["error"]["fields"][0]["path"],
        json!("inboundToken")
    );
    let mut ftp = pair(instance(PROTOCOL));
    ftp["url"] = json!("ftp://a.example");
    let unusable = two.b_http.post(Who::Guest, &path, &ftp).await;
    assert_eq!(unusable.status, 422, "{}", unusable.body);
    assert_eq!(unusable.json()["error"]["fields"][0]["path"], json!("url"));
    assert_eq!(
        unusable.json()["error"]["fields"][0]["key"],
        json!(ERROR_URL)
    );
    let mut owner = pair(instance(PROTOCOL));
    owner["inboundScopes"] = json!({ "functions": ["svc.*"], "level": "owner" });
    let above = two.b_http.post(Who::Guest, &path, &owner).await;
    assert_eq!(above.status, 422, "{}", above.body);
    assert_eq!(
        above.json()["error"]["fields"][0]["path"],
        json!("inboundScopes.level")
    );
    assert_eq!(
        two.b
            .database()
            .rows("SELECT count(*)::text FROM peer_pair_code")
            .await,
        ["1"]
    );

    two.finish().await;
}

#[tokio::test]
async fn a_pair_code_checks_its_grants_and_only_an_administrator_issues_one() {
    let harness = installation().await;
    let mask = harness
        .call(
            Who::Admin,
            PAIR_CODE,
            json!({ "scopes": ["svc.*", "svc..x"], "level": "admin" }),
        )
        .await;
    field_error(&mask, ErrorCode::Validation, "scopes[1]", ERROR_MASK);
    let owner = harness
        .call(
            Who::Admin,
            PAIR_CODE,
            json!({ "scopes": [], "level": "owner" }),
        )
        .await;
    field_error(&owner, ErrorCode::Validation, "level", ERROR_LEVEL);
    let socket = harness
        .call(
            Who::Owner,
            PAIR_CODE,
            json!({ "scopes": [], "level": "admin" }),
        )
        .await;
    assert_eq!(socket.status(), 200, "{socket:?}");
    let short = harness
        .call(Who::Admin, PAIR_CODE, json!({ "scopes": [], "ttl": 59 }))
        .await;
    assert_eq!(short.error().unwrap().code, ErrorCode::Validation);
    for who in [Who::Reseller, Who::A1] {
        let refused = harness.call(who, PAIR_CODE, json!({ "scopes": [] })).await;
        assert_eq!(refused.error().unwrap().code, ErrorCode::Forbidden);
    }
    let issued = harness
        .call(
            Who::Admin,
            PAIR_CODE,
            json!({ "scopes": ["svc.*"], "onBehalf": true }),
        )
        .await;
    let expires = data(&issued)["expiresAt"].as_str().unwrap().to_owned();
    let stored = harness
        .database()
        .rows("SELECT scopes_json::text || ' ' || (expires_at - created_at)::text || ' ' || length(code_hash)::text FROM peer_pair_code WHERE scopes_json->>'onBehalf' = 'true'")
        .await;
    assert_eq!(
        stored,
        [r#"{"level": "service", "onBehalf": true, "functions": ["svc.*"]} 00:10:00 64"#]
    );
    assert!(expires.ends_with('Z'), "{expires}");
    let code = data(&issued)["code"].as_str().unwrap().to_owned();
    let hashes = harness
        .database()
        .rows("SELECT code_hash FROM peer_pair_code")
        .await;
    assert!(!hashes.iter().any(|hash| hash.contains(&code)));

    let mut read_only = harness.principal(Who::Admin).clone();
    read_only.scope = keel_sdk::types::Scope::ReadOnly;
    let paired = harness
        .call_as_principal(
            &read_only,
            CREATE,
            json!({ "url": "http://127.0.0.1:1", "code": code }),
        )
        .await;
    error_key(&paired, ErrorCode::Forbidden, "keel.error.token_read_only");
    harness.finish().await;
}

#[tokio::test]
async fn the_daily_check_raises_a_problem_for_a_protocol_out_of_reach_and_clears_it() {
    let two = Two::start().await;
    let linked = two
        .pair(json!({ "scopes": [] }), json!({ "scopes": [] }))
        .await;
    let far = InstanceInfo {
        instance_id: two.b.instance(),
        keel: "9.0.0".into(),
        protocol: PROTOCOL + 2,
        products: vec!["dns".into()],
        public_key: two.b.instance_key().public_key(),
        features: Vec::new(),
        service_kinds: Vec::new(),
    };
    let router = axum::Router::new().route(
        "/api/v1/instance",
        axum::routing::get(move || {
            let far = far.clone();
            async move { axum::Json(far) }
        }),
    );
    let fake = TestHttp::bind(router).await.unwrap();
    two.a
        .db()
        .execute_unprepared(&format!(
            "UPDATE peer_product SET url = '{}' WHERE id = '{}'",
            fake.url(""),
            linked.a_link
        ))
        .await
        .unwrap();
    run_check(&two.a).await;
    assert_eq!(
        open_problems(&two.a).await,
        [format!("{PROBLEM_PROTOCOL} {}", linked.a_link)]
    );
    assert_eq!(
        two.a
            .database()
            .rows(&format!(
                "SELECT keel_version || ' ' || protocol::text || ' ' || products_json::text FROM peer_product WHERE id = '{}'",
                linked.a_link
            ))
            .await,
        [format!("9.0.0 {} [\"dns\"]", PROTOCOL + 2)]
    );
    let rotated = two
        .a
        .call(Who::Admin, ROTATE, json!({ "id": linked.a_link }))
        .await;
    error_key(&rotated, ErrorCode::Conflict, ERROR_PROTOCOL);

    two.a
        .db()
        .execute_unprepared(&format!(
            "UPDATE peer_product SET url = '{}' WHERE id = '{}'",
            two.b_url(),
            linked.a_link
        ))
        .await
        .unwrap();
    run_check(&two.a).await;
    assert!(open_problems(&two.a).await.is_empty());
    fake.stop().await.unwrap();
    two.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn an_initiator_with_an_address_and_events_receives_a_webhook_secret_both_sides_keep() {
    let a = composed(Harness::new().module(&keel_system::settings::MODULE))
        .boot()
        .await;
    let b = installation().await;
    let a_http = a.http().await;
    let b_http = b.http().await;
    let set = a
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::auth::SETTING_SHELL_URL, "value": format!("{}/", a_http.url("")) }),
        )
        .await;
    assert_eq!(set.status(), 200, "{set:?}");
    let first = code(&b, json!({ "scopes": [] })).await;
    let created = a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": b_http.url(""), "code": first, "subscribe": ["dns.zone.*"] }),
        )
        .await;
    assert_eq!(created.status(), 200, "{created:?}");
    let b_link = only_link(&b).await;
    let on_b = b.call(Who::Admin, GET, json!({ "id": b_link })).await;
    assert_eq!(data(&on_b)["url"], json!(a_http.url("")));
    let opened = |harness: &Harness, sealed: &str| {
        let cipher = harness
            .registry()
            .services()
            .get::<dyn SecretCipher>()
            .unwrap();
        String::from_utf8(cipher.open(sealed).unwrap()).unwrap()
    };
    let on_a = a
        .database()
        .rows("SELECT webhook_secret_enc FROM peer_product")
        .await;
    let stored_b = b
        .database()
        .rows("SELECT secret FROM webhook_subscription")
        .await;
    let secret = opened(&b, &stored_b[0]);
    assert!(secret.starts_with("whs_"), "{secret}");
    assert_eq!(opened(&a, &on_a[0]), secret);
    assert_eq!(
        b.database()
            .rows("SELECT (webhook_secret_enc IS NULL)::text FROM peer_product")
            .await,
        ["true"],
        "the side that signs keeps the secret in its subscription alone"
    );

    let bad = a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": b_http.url(""), "code": "PAIR-0000-0000", "subscribe": ["dns..zone"] }),
        )
        .await;
    field_error(&bad, ErrorCode::Validation, "subscribe[0]", ERROR_MASK);

    a_http.stop().await.unwrap();
    b_http.stop().await.unwrap();
    a.finish().await;
    b.finish().await;
}

async fn link(harness: &Harness, id: &str, url: &str, products: &str, created_at: &str) {
    harness
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO peer_product (id, instance_id, products_json, keel_version, protocol, url, \
             public_key, inbound_token_hash, inbound_scopes_json, outbound_token_enc, \
             webhook_secret_enc, status, last_seen_at, created_at) VALUES ('{id}', 'instance-{id}', \
             '{products}', '0.4.0', 1, '{url}', 'ed25519:MCowBQYDK2VwAyEA{id}', 'hash-{id}', \
             '{{\"functions\": [\"svc.*\"], \"level\": \"admin\", \"onBehalf\": true}}', \
             'sealed-token-{id}', 'sealed-secret-{id}', '{STATUS_ACTIVE}', NULL, '{created_at}')"
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_handshake_names_the_peers_feature() {
    let harness = installation().await;
    let http = harness.http().await;

    let answer = http.get(Who::Guest, "/api/v1/instance").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let info: InstanceInfo = serde_json::from_str(&answer.body).unwrap();
    assert!(
        info.features.iter().any(|feature| feature == FEATURE),
        "{info:?}"
    );
    assert_eq!(info.public_key, harness.instance_key().public_key());

    drop(http);
    harness.finish().await;
}

#[tokio::test]
async fn peer_list_and_get_answer_the_stored_links_without_their_secrets() {
    let harness = installation().await;
    link(
        &harness,
        OLDER,
        "https://dns.example:8443",
        r#"["dns"]"#,
        "2026-09-20T10:00:00Z",
    )
    .await;
    link(
        &harness,
        NEWER,
        "https://bill.example",
        r#"["bill", "dns"]"#,
        "2026-09-21T10:00:00Z",
    )
    .await;

    let listed = harness.call(Who::Admin, LIST, json!({})).await;
    assert_eq!(listed.status(), 200, "{listed:?}");
    let data = listed.data().unwrap();
    assert_eq!(data["total"], json!(2));
    let rows = data["data"].as_array().unwrap();
    let ids: Vec<&Value> = rows.iter().map(|row| &row["id"]).collect();
    assert_eq!(ids, [&json!(NEWER), &json!(OLDER)]);
    assert_eq!(
        rows[0],
        json!({
            "id": NEWER,
            "instanceId": format!("instance-{NEWER}"),
            "products": ["bill", "dns"],
            "keel": "0.4.0",
            "protocol": 1,
            "url": "https://bill.example",
            "publicKey": format!("ed25519:MCowBQYDK2VwAyEA{NEWER}"),
            "scopes": { "functions": ["svc.*"], "level": "admin", "onBehalf": true },
            "status": STATUS_ACTIVE,
            "lastSeenAt": null,
            "createdAt": "2026-09-21T10:00:00Z",
            "identity": false
        })
    );
    let text = serde_json::to_string(data).unwrap();
    for secret in ["hash-", "sealed-token-", "sealed-secret-"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }

    let got = harness.call(Who::Admin, GET, json!({ "id": OLDER })).await;
    assert_eq!(got.status(), 200, "{got:?}");
    assert_eq!(got.data().unwrap(), &rows[1]);

    let missing = harness
        .call(
            Who::Admin,
            GET,
            json!({ "id": "01J9C00000000000000000000C" }),
        )
        .await;
    assert_eq!(missing.error().unwrap().code, ErrorCode::NotFound);

    for who in [Who::Reseller, Who::A1] {
        let refused = harness.call(who, LIST, json!({})).await;
        assert_eq!(
            refused.error().unwrap().code,
            ErrorCode::Forbidden,
            "{refused:?}"
        );
    }

    harness.finish().await;
}

#[tokio::test]
async fn an_unreadable_link_answers_the_stored_error() {
    let harness = installation().await;
    link(
        &harness,
        OLDER,
        "https://dns.example:8443",
        r#"{"dns": true}"#,
        "2026-09-20T10:00:00Z",
    )
    .await;

    let got = harness.call(Who::Admin, GET, json!({ "id": OLDER })).await;
    let error = got.error().unwrap();
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.key, ERROR_STORED);

    harness.finish().await;
}

#[cfg(feature = "audit")]
#[tokio::test]
async fn pairing_and_rotation_are_audited_on_both_sides_without_their_code_or_tokens() {
    use keel_system::audit::api::Flush;

    let audited = || composed(Harness::new().module(&keel_system::audit::MODULE));
    let a = audited().boot().await;
    let b = audited().boot().await;
    let a_http = a.http().await;
    let b_http = b.http().await;
    let first = code(
        &b,
        json!({ "scopes": ["keel.account.list"], "level": "admin" }),
    )
    .await;
    let created = a
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": b_http.url(""), "code": first, "scopes": ["keel.account.list"], "level": "admin" }),
        )
        .await;
    assert_eq!(created.status(), 200, "{created:?}");
    let a_link = data(&created)["id"].as_str().unwrap().to_owned();
    let b_link = only_link(&b).await;
    let before = outbound(&a, &a_link).await;
    let rotated = a.call(Who::Admin, ROTATE, json!({ "id": a_link })).await;
    assert_eq!(rotated.status(), 200, "{rotated:?}");
    let secrets = [
        first.clone(),
        before,
        outbound(&a, &a_link).await,
        outbound(&b, &b_link).await,
    ];
    for harness in [&a, &b] {
        harness
            .registry()
            .services()
            .get::<Flush>()
            .expect("the audit module provides Flush")
            .flush()
            .await
            .expect("the worker answers the flush");
    }
    let trail = "SELECT function || ' ' || principal_login || ' ' || via || ' ' || status FROM audit_call WHERE module = 'peer' ORDER BY function";
    assert_eq!(
        a.database().rows(trail).await,
        [
            "peer.link admin internal committed",
            "peer.rekey admin internal committed",
        ]
    );
    let on_b = b.database().rows(trail).await;
    assert_eq!(
        on_b,
        [
            "peer.pair guest http committed",
            "peer.pair_code admin http committed",
            "peer.renew guest http committed",
        ]
    );
    for harness in [&a, &b] {
        let params = harness
            .database()
            .rows("SELECT params_redacted::text FROM audit_call WHERE module = 'peer'")
            .await;
        for text in &params {
            for secret in &secrets {
                assert!(!text.contains(secret.as_str()), "{text}");
            }
        }
    }
    let masked = b
        .database()
        .rows("SELECT params_redacted->>'code' || ' ' || (params_redacted->>'inboundToken') FROM audit_call WHERE function = 'peer.pair'")
        .await;
    assert_eq!(masked, ["*** ***"]);

    a_http.stop().await.unwrap();
    b_http.stop().await.unwrap();
    a.finish().await;
    b.finish().await;
}
