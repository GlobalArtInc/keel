use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{AuditEvent, AuditSink};
use keel_system::auth::{
    ABSOLUTE_HOURS, IDLE_MINUTES, MODULE, REASON_LOGOUT, REASON_REPLACED, TOKEN_PREFIX,
};
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";
const AGENT: &str = "keel-system-tests/1";
const JSON: (&str, &str) = ("content-type", "application/json");

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new().module(&MODULE).boot().await;
    let a1 = harness.principal(Who::A1).id;
    let set = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": PASSWORD }),
        )
        .await;
    assert!(set.data().is_some(), "{:?}", set.error());
    let http = harness.http().await;
    (harness, http)
}

async fn login(http: &TestHttp, login: &str, password: &str, extra: &[(&str, &str)]) -> Answer {
    let mut headers = vec![JSON, ("user-agent", AGENT)];
    headers.extend_from_slice(extra);
    http.request(
        "POST",
        "/api/v1/call/auth.login",
        &headers,
        Some(&json!({ "login": login, "password": password }).to_string()),
    )
    .await
}

async fn with_cookie(http: &TestHttp, method: &str, path: &str, cookie: &str) -> Answer {
    with_cookie_and(http, method, path, cookie, &[]).await
}

async fn with_cookie_and(
    http: &TestHttp,
    method: &str,
    path: &str,
    cookie: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut headers = vec![("cookie", cookie), JSON, ("user-agent", AGENT)];
    headers.extend_from_slice(extra);
    let body = (method == "POST").then_some("{}");
    http.request(method, path, &headers, body).await
}

fn cookie_of(answer: &Answer) -> String {
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie: {:?}", answer.headers))
}

#[tokio::test]
async fn login_sets_the_cookie_and_the_session_drives_boot_lists_and_forbidden_field() {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let answer = login(&http, "a1", PASSWORD, &[]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let body = answer.json();
    assert_eq!(body["data"]["state"], json!("authenticated"));
    assert_eq!(body["data"]["principal"]["login"], json!("a1"));
    assert_eq!(body["data"]["principal"]["id"], json!(a1.id.to_string()));
    assert_eq!(
        body["data"]["principal"]["account"],
        json!(a1.account_id.to_string())
    );
    let set_cookie = answer.set_cookie().expect("a Set-Cookie header");
    let (pair, attributes) = set_cookie.split_once("; ").expect("attributes");
    let (name, token) = pair.split_once('=').expect("a name and a value");
    assert_eq!(name, "__Host-keel_session");
    assert!(token.starts_with(TOKEN_PREFIX), "{token}");
    assert_eq!(token.len(), TOKEN_PREFIX.len() + 64);
    assert_eq!(attributes, "Path=/; Secure; HttpOnly; SameSite=Lax");
    let cookie = cookie_of(&answer);

    let booted = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    assert_eq!(booted.json()["principal"]["login"], json!("a1"));
    assert_eq!(booted.json()["principal"]["id"], json!(a1.id.to_string()));

    let accounts = with_cookie(&http, "POST", "/api/v1/call/keel.account.list", &cookie).await;
    assert_eq!(accounts.status, 200, "{}", accounts.body);
    let page = accounts.json();
    assert_eq!(page["data"]["total"], json!(1));
    assert_eq!(
        page["data"]["data"][0]["id"],
        json!(a1.account_id.to_string())
    );
    assert_eq!(page["meta"]["principal"]["login"], json!("a1"));

    let foreign = with_cookie_and(
        &http,
        "POST",
        "/api/v1/call/keel.account.list",
        &cookie,
        &[("origin", "https://evil.example")],
    )
    .await;
    assert_eq!(foreign.status, 403, "{}", foreign.body);
    assert_eq!(foreign.json()["error"]["code"], json!("forbidden"));
    assert_eq!(
        foreign.json()["error"]["key"],
        json!("keel.error.cross_site")
    );

    let above = http
        .request(
            "POST",
            "/api/v1/call/keel.principal.update",
            &[("cookie", &cookie), JSON],
            Some(&json!({ "id": a1.id, "account": harness.fixtures().accounts.a2 }).to_string()),
        )
        .await;
    assert_eq!(above.status, 403, "{}", above.body);
    assert_eq!(above.json()["error"]["code"], json!("forbidden_field"));
    assert_eq!(above.json()["error"]["fields"][0]["path"], json!("account"));

    let logout = with_cookie(&http, "POST", "/api/v1/call/auth.logout", &cookie).await;
    assert_eq!(logout.status, 200, "{}", logout.body);
    assert_eq!(logout.json()["data"]["revoked"], json!(true));
    assert_eq!(
        logout.set_cookie(),
        Some("__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    let reasons = harness
        .database()
        .rows("SELECT reason FROM auth_session WHERE revoked_at IS NOT NULL")
        .await;
    assert_eq!(reasons, [REASON_LOGOUT]);
    let gone = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(gone.status, 401, "{}", gone.body);
    assert_eq!(
        gone.json()["error"]["key"],
        json!("keel.error.unauthenticated")
    );
    let leaked = harness
        .database()
        .rows(r"SELECT count(*)::text FROM auth_session s WHERE s::text LIKE '%ks\_%'")
        .await;
    assert_eq!(leaked, ["0"]);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn login_is_refused_cross_site_and_replaces_the_session_it_arrived_with() {
    let (harness, http) = boot().await;
    let refused = login(&http, "a1", PASSWORD, &[("origin", "https://evil.example")]).await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(
        refused.json()["error"]["key"],
        json!("keel.error.cross_site")
    );
    assert_eq!(refused.set_cookie(), None);
    let fetched = login(&http, "a1", PASSWORD, &[("sec-fetch-site", "cross-site")]).await;
    assert_eq!(fetched.status, 403, "{}", fetched.body);
    assert_eq!(fetched.set_cookie(), None);

    let first = login(&http, "a1", PASSWORD, &[]).await;
    assert_eq!(first.status, 200, "{}", first.body);
    let first_cookie = cookie_of(&first);
    let second = login(&http, "a1", PASSWORD, &[("cookie", &first_cookie)]).await;
    assert_eq!(second.status, 200, "{}", second.body);
    let second_cookie = cookie_of(&second);
    assert_ne!(first_cookie, second_cookie);
    let reasons = harness
        .database()
        .rows("SELECT reason FROM auth_session WHERE revoked_at IS NOT NULL")
        .await;
    assert_eq!(reasons, [REASON_REPLACED]);
    let stale = with_cookie(&http, "GET", "/api/v1/boot", &first_cookie).await;
    assert_eq!(stale.status, 401, "{}", stale.body);
    let fresh = with_cookie(&http, "GET", "/api/v1/boot", &second_cookie).await;
    assert_eq!(fresh.status, 200, "{}", fresh.body);
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

fn without_request_id(mut body: Value) -> Value {
    if let Some(error) = body.get_mut("error").and_then(Value::as_object_mut) {
        error.remove("requestId");
    }
    body
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_login_are_one_answer_and_the_audit_masks_the_password() {
    let harness = Harness::new().module(&MODULE).module(&AUDIT).boot().await;
    let a1 = harness.principal(Who::A1).id;
    harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": PASSWORD }),
        )
        .await;
    let http = harness.http().await;
    let wrong = login(&http, "a1", "not-it", &[]).await;
    let unknown = login(&http, "nobody", PASSWORD, &[]).await;
    assert_eq!(wrong.status, 401, "{}", wrong.body);
    assert_eq!(unknown.status, 401, "{}", unknown.body);
    assert_eq!(wrong.json()["error"]["code"], json!("unauthenticated"));
    assert_eq!(
        wrong.json()["error"]["key"],
        json!("auth.error.credentials")
    );
    assert_eq!(
        without_request_id(wrong.json()),
        without_request_id(unknown.json())
    );
    assert_eq!(wrong.set_cookie(), None);
    let events: Vec<AuditEvent> = RECORDER
        .events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|event| event.function == "auth.login")
        .cloned()
        .collect();
    assert_eq!(events.len(), 2, "{events:?}");
    for event in &events {
        assert_eq!(event.module, "auth");
        assert_eq!(event.params["password"], json!("***"));
        assert!(!event.outcome.is_committed(), "{event:?}");
        assert_eq!(event.principal.level, Level::Guest);
    }
    assert_eq!(events[0].params["login"], json!("a1"));
    assert_eq!(events[1].params["login"], json!("nobody"));
    let recorded = serde_json::to_string(&events).expect("events serialise");
    assert!(!recorded.contains(PASSWORD));
    assert!(!recorded.contains("not-it"));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_session_slides_with_every_request_and_ends_after_the_idle_and_absolute_lifetimes() {
    let (harness, http) = boot().await;
    let minutes = |n: i64| keel_sdk::chrono::Duration::minutes(n);
    let idle = login(&http, "a1", PASSWORD, &[]).await;
    let cookie = cookie_of(&idle);
    let recorded = harness
        .database()
        .rows("SELECT ip || ' ' || user_agent FROM auth_session")
        .await;
    assert_eq!(recorded, [format!("127.0.0.1 {AGENT}")]);
    harness.clock().advance(minutes(IDLE_MINUTES - 1));
    let alive = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(alive.status, 200, "{}", alive.body);
    harness.clock().advance(minutes(IDLE_MINUTES - 1));
    let slid = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(slid.status, 200, "{}", slid.body);
    harness.clock().advance(minutes(IDLE_MINUTES + 1));
    let expired = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(expired.status, 401, "{}", expired.body);

    let fresh = login(&http, "a1", PASSWORD, &[]).await;
    let cookie = cookie_of(&fresh);
    let step = 10;
    let steps = ABSOLUTE_HOURS * 60 / step;
    for tick in 1..steps {
        harness.clock().advance(minutes(step));
        let answer = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
        assert_eq!(
            answer.status,
            200,
            "after {} minutes: {}",
            tick * step,
            answer.body
        );
    }
    harness.clock().advance(minutes(step));
    let absolute = with_cookie(&http, "GET", "/api/v1/boot", &cookie).await;
    assert_eq!(absolute.status, 401, "{}", absolute.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn bearer_tokens_are_not_sessions_and_the_test_principal_header_still_works() {
    let (harness, http) = boot().await;
    let answer = login(&http, "a1", PASSWORD, &[]).await;
    let cookie = cookie_of(&answer);
    let token = cookie.split_once('=').expect("a value").1;
    for bearer in [token, "kat_not_yet"] {
        let refused = http
            .request(
                "GET",
                "/api/v1/boot",
                &[("authorization", &format!("Bearer {bearer}"))],
                None,
            )
            .await;
        assert_eq!(refused.status, 401, "{bearer}: {}", refused.body);
        assert_eq!(
            refused.json()["error"]["key"],
            json!("keel.error.unauthenticated")
        );
    }
    let header = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(header.status, 200, "{}", header.body);
    assert_eq!(header.json()["principal"]["login"], json!("a1"));
    let no_session = http
        .post(Who::A1, "/api/v1/call/auth.logout", &json!({}))
        .await;
    assert_eq!(no_session.status, 422, "{}", no_session.body);
    assert_eq!(
        no_session.json()["error"]["key"],
        json!("auth.error.session_only")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_login_needs_http_and_the_logout_needs_a_session_over_the_socket() {
    let (harness, http) = boot().await;
    let socket = harness.socket().await;
    let login = socket
        .call("auth.login", json!({ "login": "a1", "password": PASSWORD }))
        .await;
    let login = serde_json::to_value(&login).expect("an envelope");
    assert_eq!(login["ok"], json!(false), "{login}");
    assert_eq!(login["error"]["code"], json!("validation"));
    assert_eq!(login["error"]["key"], json!("auth.error.http_only"));
    let logout = socket.call("auth.logout", json!({})).await;
    let logout = serde_json::to_value(&logout).expect("an envelope");
    assert_eq!(logout["error"]["code"], json!("validation"));
    assert_eq!(logout["error"]["key"], json!("auth.error.session_only"));
    let sessions = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_session")
        .await;
    assert_eq!(sessions, ["0"]);
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn an_unknown_login_method_is_a_validation_error_on_the_method_field() {
    let (harness, http) = boot().await;
    let answer = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": "a1", "password": PASSWORD, "method": "oidc" }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 422, "{}", answer.body);
    assert_eq!(answer.json()["error"]["fields"][0]["path"], json!("method"));
    assert_eq!(
        answer.json()["error"]["fields"][0]["key"],
        json!("auth.error.method_unknown")
    );
    let local = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": "a1", "password": PASSWORD, "method": "local" }).to_string()),
        )
        .await;
    assert_eq!(local.status, 200, "{}", local.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_login_and_logout_descriptors_match_their_snapshots_and_the_password_is_secret() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let names = harness.snapshot_descriptors(Level::Guest, snapshots_dir());
    assert_eq!(names, ["auth.key.redeem", "auth.login", "auth.totp.verify"]);
    let names = harness.snapshot_descriptors(Level::Service, snapshots_dir());
    assert_eq!(
        names,
        [
            "auth.key.redeem",
            "auth.login",
            "auth.logout",
            "auth.su.exit",
            "auth.totp.verify"
        ]
    );
    let login = harness
        .registry()
        .function("auth.login")
        .expect("the login is registered");
    let params = serde_json::to_value(&login.descriptor().params).expect("a schema");
    assert_eq!(
        params["properties"]["password"]["x-keel"]["secret"],
        json!(true)
    );
    assert_eq!(
        params["properties"]["password"]["x-ui"]["control"],
        json!("password")
    );
    harness.finish().await;
}
