use std::path::{Path, PathBuf};

use keel_sdk::chrono::Duration;
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::types::{Scope, Via};
use keel_system::auth::{API_TOKEN_PREFIX, MODULE};
use keel_testkit::auth::PRINCIPAL_HEADER;
use keel_testkit::{Answer, Fixture, Harness, TestHttp, Who, tokio};

const JSON: (&str, &str) = ("content-type", "application/json");

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).boot().await;
    let http = harness.http().await;
    (harness, http)
}

async fn issue(harness: &Harness, who: Who, params: Value) -> (Id, String) {
    let reply = harness.call(who, "auth.token.create", params).await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.token.create by {who} failed: {:?}", reply.error()));
    let id = data["id"].as_str().expect("an id").parse().expect("a ulid");
    let token = data["token"].as_str().expect("the plaintext").to_owned();
    assert!(token.starts_with(API_TOKEN_PREFIX), "{token}");
    assert_eq!(token.len(), API_TOKEN_PREFIX.len() + 64);
    (id, token)
}

async fn bearer(
    http: &TestHttp,
    method: &str,
    func: &str,
    token: &str,
    body: Option<&Value>,
    extra: &[(&str, &str)],
) -> Answer {
    let authorization = format!("Bearer {token}");
    let mut headers = vec![("authorization", authorization.as_str()), JSON];
    headers.extend_from_slice(extra);
    let body = body.map(Value::to_string);
    http.request(
        method,
        &format!("/api/v1/call/{func}"),
        &headers,
        body.as_deref(),
    )
    .await
}

async fn post(http: &TestHttp, func: &str, token: &str, body: Value) -> Answer {
    bearer(http, "POST", func, token, Some(&body), &[]).await
}

async fn keyed(http: &TestHttp, key: &str, body: &Value) -> Answer {
    http.request(
        "POST",
        "/api/v1/call/auth.token.create",
        &[
            (PRINCIPAL_HEADER, Who::A1.login()),
            ("idempotency-key", key),
            JSON,
        ],
        Some(&body.to_string()),
    )
    .await
}

fn error_of(answer: &Answer, status: u16, code: &str, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["error"]["code"], json!(code), "{}", answer.body);
    assert_eq!(body["error"]["key"], json!(key), "{}", answer.body);
}

struct TokenFixture;

#[async_trait]
impl Fixture for TokenFixture {
    async fn create(&self, harness: &Harness, account: Id) -> Id {
        let who = harness
            .fixtures()
            .all()
            .find(|(who, principal)| {
                principal.account_id == account && *who != Who::Guest && *who != Who::Owner
            })
            .map(|(who, _)| who)
            .unwrap_or_else(|| panic!("no fixture principal lives in account {account}"));
        issue(
            harness,
            who,
            json!({ "name": format!("token of {account}") }),
        )
        .await
        .0
    }
}

#[tokio::test]
async fn a_token_scoped_to_one_function_is_admitted_there_and_refused_elsewhere_after_the_level_guard()
 {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let (_, listing) = issue(
        &harness,
        Who::A1,
        json!({ "name": "listing", "scope": { "functions": ["keel.account.list"] } }),
    )
    .await;

    let accounts = post(&http, "keel.account.list", &listing, json!({})).await;
    assert_eq!(accounts.status, 200, "{}", accounts.body);
    let page = accounts.json();
    assert_eq!(page["data"]["total"], json!(1));
    assert_eq!(
        page["data"]["data"][0]["id"],
        json!(a1.account_id.to_string())
    );
    assert_eq!(page["meta"]["principal"]["login"], json!("a1"));
    assert_eq!(page["meta"]["principal"]["level"], json!("user"));
    let via_get = bearer(&http, "GET", "keel.account.list", &listing, None, &[]).await;
    assert_eq!(via_get.status, 200, "{}", via_get.body);

    let outside = post(&http, "keel.principal.list", &listing, json!({})).await;
    error_of(&outside, 403, "forbidden", "keel.error.token_scope");
    assert_eq!(
        outside.json()["error"]["params"]["function"],
        json!("keel.principal.list")
    );

    let (_, read_only) = issue(
        &harness,
        Who::A1,
        json!({ "name": "reader", "scope": "read_only" }),
    )
    .await;
    let reads = post(&http, "keel.principal.list", &read_only, json!({})).await;
    assert_eq!(reads.status, 200, "{}", reads.body);
    let writes = post(
        &http,
        "keel.principal.update",
        &read_only,
        json!({ "id": a1.id, "lang": "ru" }),
    )
    .await;
    error_of(&writes, 403, "forbidden", "keel.error.token_read_only");
    assert_eq!(
        writes.json()["error"]["params"]["function"],
        json!("keel.principal.update")
    );

    let (_, service) = issue(
        &harness,
        Who::A1,
        json!({ "name": "service", "level": "service", "scope": { "functions": ["keel.account.list"] } }),
    )
    .await;
    let below = post(&http, "keel.principal.list", &service, json!({})).await;
    error_of(&below, 403, "forbidden", "keel.error.level");
    assert_eq!(below.json()["error"]["params"]["level"], json!("user"));
    let booted = http
        .request(
            "GET",
            "/api/v1/boot",
            &[("authorization", &format!("Bearer {service}"))],
            None,
        )
        .await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["level"], json!("service"));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_token_cannot_create_a_token_wider_than_itself_and_has_no_session() {
    let (harness, http) = boot().await;
    let (creator_id, creator) = issue(
        &harness,
        Who::A1,
        json!({ "name": "creator", "scope": { "functions": ["auth.token.create"] } }),
    )
    .await;
    let full = post(
        &http,
        "auth.token.create",
        &creator,
        json!({ "name": "full", "scope": "full" }),
    )
    .await;
    error_of(&full, 403, "forbidden", "auth.error.token_scope");
    let keel = post(
        &http,
        "auth.token.create",
        &creator,
        json!({ "name": "keel", "scope": { "functions": ["keel.*"] } }),
    )
    .await;
    error_of(&keel, 403, "forbidden", "auth.error.token_scope");
    let read_only = post(
        &http,
        "auth.token.create",
        &creator,
        json!({ "name": "reader", "scope": "read_only" }),
    )
    .await;
    error_of(&read_only, 403, "forbidden", "auth.error.token_scope");
    let same = post(
        &http,
        "auth.token.create",
        &creator,
        json!({ "name": "same", "scope": { "functions": ["auth.token.create"] } }),
    )
    .await;
    assert_eq!(same.status, 200, "{}", same.body);
    let inherited = post(
        &http,
        "auth.token.create",
        &creator,
        json!({ "name": "inherited" }),
    )
    .await;
    assert_eq!(inherited.status, 200, "{}", inherited.body);
    let inherited_id = inherited.json()["data"]["id"].clone();

    let (_, wide) = issue(
        &harness,
        Who::A1,
        json!({ "name": "wide", "scope": { "functions": ["keel.*", "auth.token.create"] } }),
    )
    .await;
    let narrower = post(
        &http,
        "auth.token.create",
        &wide,
        json!({ "name": "narrower", "scope": { "functions": ["keel.account.list", "keel.account.*"] } }),
    )
    .await;
    assert_eq!(narrower.status, 200, "{}", narrower.body);
    let auth = post(
        &http,
        "auth.token.create",
        &wide,
        json!({ "name": "auth", "scope": { "functions": ["auth.*"] } }),
    )
    .await;
    error_of(&auth, 403, "forbidden", "auth.error.token_scope");
    let wider_prefix = post(
        &http,
        "auth.token.create",
        &wide,
        json!({ "name": "prefix", "scope": { "functions": ["auth.token.*"] } }),
    )
    .await;
    error_of(&wider_prefix, 403, "forbidden", "auth.error.token_scope");

    let (_, reader) = issue(
        &harness,
        Who::A1,
        json!({ "name": "reader", "scope": "read_only" }),
    )
    .await;
    let never = post(&http, "auth.token.create", &reader, json!({ "name": "x" })).await;
    error_of(&never, 403, "forbidden", "keel.error.token_read_only");

    let (_, full) = issue(&harness, Who::A1, json!({ "name": "full" })).await;
    let logout = post(&http, "auth.logout", &full, json!({})).await;
    error_of(&logout, 422, "validation", "auth.error.session_only");
    let logout = post(&http, "auth.logout", &wide, json!({})).await;
    error_of(&logout, 403, "forbidden", "keel.error.token_scope");

    let listed = harness.call(Who::A1, "auth.token.list", json!({})).await;
    let rows = listed.data().expect("the list answers")["data"]
        .as_array()
        .expect("rows")
        .clone();
    let scope_of = |name: &str| {
        rows.iter()
            .find(|row| row["name"] == json!(name))
            .map(|row| row["scope"].clone())
            .unwrap_or_else(|| panic!("no token named {name}: {rows:?}"))
    };
    assert_eq!(
        scope_of("inherited"),
        json!({ "functions": ["auth.token.create"] })
    );
    assert_eq!(
        scope_of("creator"),
        json!({ "functions": ["auth.token.create"] })
    );
    assert_eq!(scope_of("reader"), json!("read_only"));
    assert!(
        rows.iter()
            .any(|row| row["id"] == json!(creator_id.to_string()) && row["id"] != inherited_id)
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn ip_rules_revocation_expiry_and_last_used_govern_a_token_and_the_plaintext_is_shown_once() {
    let (harness, http) = boot().await;
    let (_, elsewhere) = issue(
        &harness,
        Who::A1,
        json!({ "name": "elsewhere", "ipAllow": ["10.0.0.0/8"] }),
    )
    .await;
    let refused = post(&http, "keel.account.list", &elsewhere, json!({})).await;
    error_of(
        &refused,
        401,
        "unauthenticated",
        "keel.error.unauthenticated",
    );
    assert_eq!(refused.set_cookie(), None);
    let (_, local) = issue(
        &harness,
        Who::A1,
        json!({ "name": "local", "ipAllow": ["10.0.0.0/8", "127.0.0.0/8"] }),
    )
    .await;
    let admitted = post(&http, "keel.account.list", &local, json!({})).await;
    assert_eq!(admitted.status, 200, "{}", admitted.body);
    let malformed = harness
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "bad", "ipAllow": ["127.0.0.1"] }),
        )
        .await;
    let field = &malformed.error().expect("a validation error").fields[0];
    assert_eq!(malformed.status(), 422);
    assert_eq!(field.path, "ipAllow[0]");
    assert_eq!(field.code.as_deref(), Some("check.cidr"));

    let (id, token) = issue(&harness, Who::A1, json!({ "name": "once" })).await;
    let body = json!({ "name": "keyed" });
    let created = keyed(&http, "token-once", &body).await;
    assert_eq!(created.status, 200, "{}", created.body);
    let shown = created.json()["data"]["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned();
    assert!(shown.starts_with(API_TOKEN_PREFIX), "{shown}");
    let replayed = keyed(&http, "token-once", &body).await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.header("x-keel-idempotent-replay"), Some("true"));
    assert_eq!(replayed.json()["data"]["token"], json!("***"));
    assert_eq!(replayed.json()["data"]["id"], created.json()["data"]["id"]);
    let works = post(&http, "keel.account.list", &shown, json!({})).await;
    assert_eq!(works.status, 200, "{}", works.body);
    let stored = harness
        .database()
        .rows(r"SELECT count(*)::text FROM auth_token t WHERE t::text LIKE '%kat\_%'")
        .await;
    assert_eq!(stored, ["0"]);
    let recorded = harness
        .database()
        .rows(
            r"SELECT count(*)::text FROM keel_idempotency WHERE response_json::text LIKE '%kat\_%'",
        )
        .await;
    assert_eq!(recorded, ["0"]);
    let masked = harness
        .database()
        .rows("SELECT response_json->>'token' FROM keel_idempotency WHERE key = 'token-once'")
        .await;
    assert_eq!(masked, ["***"]);
    let listed = harness.call(Who::A1, "auth.token.list", json!({})).await;
    let text = serde_json::to_string(listed.data().expect("the list answers")).expect("json");
    assert!(!text.contains(&token));
    assert!(!text.contains("tokenHash"));
    let unused = harness
        .database()
        .rows(&format!(
            "SELECT (last_used_at IS NULL)::text FROM auth_token WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(unused, ["true"]);
    let used = post(&http, "keel.account.list", &token, json!({})).await;
    assert_eq!(used.status, 200, "{}", used.body);
    let stamped = harness
        .database()
        .rows(&format!(
            "SELECT (last_used_at = '{}')::text FROM auth_token WHERE id = '{id}'",
            harness.clock().now().to_rfc3339()
        ))
        .await;
    assert_eq!(stamped, ["true"]);
    let revoked = harness
        .call(Who::A1, "auth.token.revoke", json!({ "id": id }))
        .await;
    assert_eq!(
        revoked.data().map(|data| data["revoked"].clone()),
        Some(json!(true)),
        "{:?}",
        revoked.error()
    );
    let after = post(&http, "keel.account.list", &token, json!({})).await;
    error_of(&after, 401, "unauthenticated", "keel.error.unauthenticated");
    let again = harness
        .call(Who::A1, "auth.token.revoke", json!({ "id": id }))
        .await;
    assert_eq!(again.data().expect("idempotent")["revoked"], json!(false));

    let expires_at = harness.clock().now() + Duration::hours(1);
    let (_, expiring) = issue(
        &harness,
        Who::A1,
        json!({ "name": "expiring", "expiresAt": expires_at }),
    )
    .await;
    let alive = post(&http, "keel.account.list", &expiring, json!({})).await;
    assert_eq!(alive.status, 200, "{}", alive.body);
    harness.clock().advance(Duration::minutes(61));
    let expired = post(&http, "keel.account.list", &expiring, json!({})).await;
    error_of(
        &expired,
        401,
        "unauthenticated",
        "keel.error.unauthenticated",
    );
    let past = harness
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "past", "expiresAt": harness.clock().now() - Duration::seconds(1) }),
        )
        .await;
    assert_eq!(past.status(), 422);
    assert_eq!(
        past.error().expect("validation").fields[0].path,
        "expiresAt"
    );
    assert_eq!(
        past.error().expect("validation").fields[0].key.as_deref(),
        Some("auth.error.token_expires_past")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_ceiling_the_masks_and_the_owner_are_checked_on_create() {
    let (harness, http) = boot().await;
    for level in ["reseller", "guest"] {
        let above = harness
            .call(
                Who::A1,
                "auth.token.create",
                json!({ "name": "above", "level": level }),
            )
            .await;
        assert_eq!(above.status(), 422, "{level}: {:?}", above.envelope);
        let field = &above.error().expect("validation").fields[0];
        assert_eq!(field.path, "level");
        assert_eq!(field.key.as_deref(), Some("auth.error.token_level"));
    }
    let (_, lowered) = issue(
        &harness,
        Who::Reseller,
        json!({ "name": "lowered", "level": "user" }),
    )
    .await;
    let raised = post(
        &http,
        "auth.token.create",
        &lowered,
        json!({ "name": "raised", "level": "reseller" }),
    )
    .await;
    assert_eq!(raised.status, 422, "{}", raised.body);
    assert_eq!(
        raised.json()["error"]["fields"][0]["key"],
        json!("auth.error.token_level")
    );
    let kept = post(
        &http,
        "auth.token.create",
        &lowered,
        json!({ "name": "kept", "level": "user" }),
    )
    .await;
    assert_eq!(kept.status, 200, "{}", kept.body);

    let masks = harness
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "masks", "scope": { "functions": ["keel.account.list", "keel.*.list"] } }),
        )
        .await;
    assert_eq!(masks.status(), 422, "{:?}", masks.envelope);
    let field = &masks.error().expect("validation").fields[0];
    assert_eq!(field.path, "scope.functions[1]");
    assert_eq!(field.key.as_deref(), Some("auth.error.token_mask"));
    assert_eq!(field.params["mask"], json!("keel.*.list"));

    let socket = harness.socket().await;
    let owner = socket
        .call("auth.token.create", json!({ "name": "owner" }))
        .await;
    let owner = serde_json::to_value(&owner).expect("an envelope");
    assert_eq!(owner["error"]["code"], json!("forbidden"), "{owner}");
    assert_eq!(owner["error"]["key"], json!("auth.error.token_owner"));
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_bearer_request_with_a_foreign_origin_is_not_cross_site() {
    let (harness, http) = boot().await;
    let (_, token) = issue(&harness, Who::A1, json!({ "name": "api" })).await;
    let foreign = bearer(
        &http,
        "POST",
        "keel.account.list",
        &token,
        Some(&json!({})),
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(foreign.status, 200, "{}", foreign.body);
    let fetched = bearer(
        &http,
        "POST",
        "keel.account.list",
        &token,
        Some(&json!({})),
        &[("sec-fetch-site", "cross-site")],
    )
    .await;
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn tokens_are_the_credential_of_their_principal_and_the_standard_invariants_hold() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let created = harness
        .call(
            Who::Admin,
            "keel.principal.create",
            json!({ "login": "a1b", "level": "user", "account": a1.account_id }),
        )
        .await;
    let a1b = Principal {
        id: created.data().expect("the principal is created")["id"]
            .as_str()
            .expect("an id")
            .parse()
            .expect("a ulid"),
        login: "a1b".into(),
        level: Level::User,
        account_id: a1.account_id,
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: "en".into(),
    };
    let (id, _) = issue(&harness, Who::A1, json!({ "name": "mine" })).await;
    let mine = harness.call(Who::A1, "auth.token.list", json!({})).await;
    assert_eq!(mine.data().expect("a page")["total"], json!(1));
    let theirs = harness
        .call_as_principal(&a1b, "auth.token.list", json!({}))
        .await;
    assert_eq!(theirs.data().expect("a page")["total"], json!(0));
    let revoke = harness
        .call_as_principal(&a1b, "auth.token.revoke", json!({ "id": id }))
        .await;
    assert_eq!(revoke.status(), 404, "{:?}", revoke.envelope);
    assert_eq!(
        revoke.error().expect("not found").key,
        "keel.error.not_found"
    );
    let reseller = harness
        .call(Who::Reseller, "auth.token.list", json!({}))
        .await;
    assert_eq!(reseller.data().expect("a page")["total"], json!(1));
    assert_eq!(
        reseller.data().expect("a page")["data"][0]["principalId"],
        json!(a1.id.to_string())
    );
    let revoked = harness
        .call(Who::Reseller, "auth.token.revoke", json!({ "id": id }))
        .await;
    assert_eq!(
        revoked
            .data()
            .expect("the reseller revokes inside its subtree")["revoked"],
        json!(true)
    );
    harness
        .assert_reseller_sees_only_subtree("auth.token.list", &TokenFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row("auth.token.revoke", &TokenFixture)
        .await;
    let names = harness.snapshot_descriptors(Level::User, snapshots_dir());
    assert!(names.contains(&"auth.token.list".to_owned()), "{names:?}");
    assert!(names.contains(&"auth.token.create".to_owned()), "{names:?}");
    assert!(names.contains(&"auth.token.revoke".to_owned()), "{names:?}");
    harness.finish().await;
}
