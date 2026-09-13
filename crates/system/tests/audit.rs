use std::path::{Path, PathBuf};
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink, Client};
use keel_system::audit::api::Flush;
use keel_system::audit::{
    BATCH_ROWS, FUNCTIONS, MENU_PATH, MODULE, STATUS_COMMITTED, STATUS_FAILED,
};
use keel_system::auth;
use keel_system::auth::{API_TOKEN_PREFIX, LOGIN_KEY_PREFIX, TOKEN_PREFIX, totp_code, totp_step};
use keel_testkit::{Answer, CallOptions, Fixture, Harness, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";
const JSON: (&str, &str) = ("content-type", "application/json");
const SEEDED_AT: &str = "2026-09-13T19:59:59Z";
const RECENT: &str = "at > '2026-09-13T19:59:59Z'";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn set_password(harness: &Harness, id: Id) {
    let set = harness
        .call(
            Who::Owner,
            "auth.password.set",
            json!({ "principal": id, "password": PASSWORD }),
        )
        .await;
    assert!(set.data().is_some(), "{id}: {:?}", set.error());
}

fn seeded_at() -> DateTime<Utc> {
    SEEDED_AT.parse().expect("an instant")
}

async fn boot() -> (Harness, TestHttp) {
    let harness = Harness::new()
        .module(&auth::MODULE)
        .module(&MODULE)
        .clock(seeded_at())
        .boot()
        .await;
    for who in [Who::Admin, Who::Reseller, Who::A1, Who::A2] {
        set_password(&harness, harness.principal(who).id).await;
    }
    harness
        .clock()
        .advance(keel_sdk::chrono::Duration::seconds(1));
    let http = harness.http().await;
    (harness, http)
}

async fn login(http: &TestHttp, who: Who) -> String {
    let answer = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": who.login(), "password": PASSWORD }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 200, "{who}: {}", answer.body);
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie for {who}: {:?}", answer.headers))
}

async fn call(http: &TestHttp, cookie: &str, func: &str, params: Value) -> Answer {
    call_with(http, cookie, func, params, &[]).await
}

async fn call_with(
    http: &TestHttp,
    cookie: &str,
    func: &str,
    params: Value,
    extra: &[(&str, &str)],
) -> Answer {
    let mut headers = vec![("cookie", cookie), JSON];
    headers.extend_from_slice(extra);
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &headers,
        Some(&params.to_string()),
    )
    .await
}

async fn flush(harness: &Harness) {
    harness
        .registry()
        .services()
        .get::<Flush>()
        .expect("the audit module provides Flush")
        .flush()
        .await
        .expect("the worker answers the flush");
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

const TRAIL: &str = "SELECT function || ' ' || principal_login || ' ' || coalesce(actor_login, '-') || ' ' || via || ' ' || status FROM audit_call WHERE at > '2026-09-13T19:59:59Z' ORDER BY 1";

fn error_of(answer: &Answer, status: u16, key: &str) {
    assert_eq!(answer.status, status, "{}", answer.body);
    assert_eq!(answer.json()["error"]["key"], json!(key), "{}", answer.body);
}

#[tokio::test]
async fn the_su_sequence_records_both_sides_of_su_and_the_calls_made_inside_it() {
    let (harness, http) = boot().await;
    let admin = harness.principal(Who::Admin).clone();
    let a1 = harness.principal(Who::A1).clone();
    let cookie = login(&http, Who::Admin).await;

    let entered = call(&http, &cookie, "auth.su", json!({ "principal": a1.id })).await;
    assert_eq!(entered.status, 200, "{}", entered.body);
    let inside = call(
        &http,
        &cookie,
        "auth.token.create",
        json!({ "name": "inside" }),
    )
    .await;
    assert_eq!(inside.status, 200, "{}", inside.body);
    assert_eq!(inside.json()["meta"]["principal"]["login"], json!("a1"));
    let exited = call(&http, &cookie, "auth.su.exit", json!({})).await;
    assert_eq!(exited.status, 200, "{}", exited.body);
    flush(&harness).await;

    assert_eq!(
        rows(&harness, TRAIL).await,
        [
            "auth.login guest - http committed",
            "auth.su admin - http committed",
            "auth.su.exit a1 admin http committed",
            "auth.token.create a1 admin http committed",
        ]
    );
    let sides = rows(
        &harness,
        &format!(
            "SELECT function || ' ' || coalesce(principal_id, '-') || ' ' || account_id || ' ' || coalesce(actor_principal_id, '-') || ' ' || params_redacted::text FROM audit_call WHERE {RECENT} AND function IN ('auth.su', 'auth.token.create') ORDER BY 1"
        ),
    )
    .await;
    assert_eq!(
        sides,
        [
            format!(
                "auth.su {} {} - {{\"principal\": \"{}\"}}",
                admin.id, admin.account_id, a1.id
            ),
            format!(
                "auth.token.create {} {} {} {{\"name\": \"inside\"}}",
                a1.id, a1.account_id, admin.id
            ),
        ]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn logins_and_password_changes_are_masked_and_no_session_token_key_or_factor_reaches_the_trail()
 {
    let (harness, http) = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let wrong = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": "a1", "password": "wrong-horse" }).to_string()),
        )
        .await;
    error_of(&wrong, 401, "auth.error.credentials");
    let cookie = login(&http, Who::A1).await;
    let changed = call(
        &http,
        &cookie,
        "auth.password.set",
        json!({ "current": PASSWORD, "password": "another-horse-42" }),
    )
    .await;
    assert_eq!(changed.status, 200, "{}", changed.body);

    let token = call(
        &http,
        &cookie,
        "auth.token.create",
        json!({ "name": "api" }),
    )
    .await;
    assert_eq!(token.status, 200, "{}", token.body);
    let plaintext = token.json()["data"]["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned();
    assert!(plaintext.starts_with(API_TOKEN_PREFIX));

    let issued = harness
        .call(
            Who::Owner,
            "auth.key.issue",
            json!({ "principal": a1.id, "ttl": 60, "redirect": "/f/note.list" }),
        )
        .await;
    let key = issued
        .data()
        .unwrap_or_else(|| panic!("auth.key.issue: {:?}", issued.error()))["key"]
        .as_str()
        .expect("a key")
        .to_owned();
    assert!(key.starts_with(LOGIN_KEY_PREFIX));
    let redeemed = http
        .request(
            "POST",
            "/api/v1/call/auth.key.redeem",
            &[JSON],
            Some(&json!({ "key": key }).to_string()),
        )
        .await;
    assert_eq!(redeemed.status, 200, "{}", redeemed.body);

    let enrolled = call(&http, &cookie, "auth.totp.enroll", json!({})).await;
    assert_eq!(enrolled.status, 200, "{}", enrolled.body);
    let secret = enrolled.json()["data"]["secret"]
        .as_str()
        .expect("a secret")
        .to_owned();
    let code = totp_code(
        &auth::decode_totp_secret(&secret).expect("a base32 secret"),
        totp_step(harness.clock().now()),
    )
    .expect("a code");
    let confirmed = call(&http, &cookie, "auth.totp.confirm", json!({ "code": code })).await;
    assert_eq!(confirmed.status, 200, "{}", confirmed.body);
    let recovery = confirmed.json()["data"]["recoveryCodes"][0]
        .as_str()
        .expect("a recovery code")
        .to_owned();
    let challenged = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": "a1", "password": "another-horse-42" }).to_string()),
        )
        .await;
    assert_eq!(challenged.status, 200, "{}", challenged.body);
    assert_eq!(challenged.json()["data"]["state"], json!("totp"));
    let challenge = challenged.json()["data"]["challenge"]
        .as_str()
        .expect("a challenge")
        .to_owned();
    let verified = http
        .request(
            "POST",
            "/api/v1/call/auth.totp.verify",
            &[JSON],
            Some(&json!({ "challenge": challenge, "code": recovery }).to_string()),
        )
        .await;
    assert_eq!(verified.status, 200, "{}", verified.body);
    flush(&harness).await;

    let masked = rows(
        &harness,
        &format!(
            "SELECT function || ' ' || status || ' ' || coalesce(error_key, '-') || ' ' || principal_login || ' ' || coalesce(principal_id, '-') || ' ' || account_id || ' ' || params_redacted::text FROM audit_call WHERE {RECENT} AND function IN ('auth.login', 'auth.password.set') ORDER BY status, params_redacted::text"
        ),
    )
    .await;
    assert_eq!(
        masked,
        [
            format!(
                "auth.password.set committed - a1 {} {} {{\"current\": \"***\", \"password\": \"***\"}}",
                a1.id, a1.account_id
            ),
            format!(
                "auth.login committed - guest - {} {{\"login\": \"a1\", \"password\": \"***\"}}",
                Id::ROOT_ACCOUNT
            ),
            format!(
                "auth.login committed - guest - {} {{\"login\": \"a1\", \"password\": \"***\"}}",
                Id::ROOT_ACCOUNT
            ),
            format!(
                "auth.login failed auth.error.credentials guest - {} {{\"login\": \"a1\", \"password\": \"***\"}}",
                Id::ROOT_ACCOUNT
            ),
        ]
    );
    let functions = rows(
        &harness,
        &format!(
            "SELECT function || ' ' || status || ' ' || coalesce(after::text, '-') || ' ' || coalesce(subject, '-') FROM audit_call WHERE {RECENT} AND function NOT IN ('auth.login', 'auth.password.set') ORDER BY 1"
        ),
    )
    .await;
    assert_eq!(
        functions,
        [
            "auth.key.issue committed - -",
            "auth.key.redeem committed - -",
            "auth.token.create committed - -",
            "auth.totp.confirm committed - -",
            "auth.totp.enroll committed - -",
            "auth.totp.verify committed - -",
        ]
    );
    for needle in [
        TOKEN_PREFIX,
        API_TOKEN_PREFIX,
        LOGIN_KEY_PREFIX,
        PASSWORD,
        "another-horse-42",
        "wrong-horse",
        secret.as_str(),
        recovery.as_str(),
        challenge.as_str(),
        plaintext.as_str(),
        key.as_str(),
    ] {
        let leaked = rows(
            &harness,
            &format!(
                "SELECT count(*)::text FROM audit_call WHERE params_redacted::text LIKE '%{needle}%' OR coalesce(after::text, '') LIKE '%{needle}%' OR coalesce(subject, '') LIKE '%{needle}%'"
            ),
        )
        .await;
        assert_eq!(leaked, ["0"], "{needle} reached the trail");
    }
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_read_leaves_no_row_a_replay_leaves_no_second_row_and_request_id_ip_and_via_are_filled() {
    let (harness, http) = boot().await;
    let cookie = login(&http, Who::A1).await;
    let read = call(&http, &cookie, "keel.account.list", json!({})).await;
    assert_eq!(read.status, 200, "{}", read.body);
    let first = call_with(
        &http,
        &cookie,
        "auth.token.create",
        json!({ "name": "keyed" }),
        &[("idempotency-key", "token-keyed")],
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.body);
    let request_id = first.json()["meta"]["requestId"]
        .as_str()
        .expect("a request id")
        .to_owned();
    let replay = call_with(
        &http,
        &cookie,
        "auth.token.create",
        json!({ "name": "keyed" }),
        &[("idempotency-key", "token-keyed")],
    )
    .await;
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert_eq!(replay.header("x-keel-idempotent-replay"), Some("true"));
    assert_eq!(replay.json()["data"]["token"], json!("***"));

    let socket = harness.socket().await;
    let created = socket
        .call("keel.account.create", json!({ "kind": "user" }))
        .await;
    assert_eq!(created.status(), 200, "{created:?}");
    let delegated = socket
        .call_as("auth.token.create", json!({ "name": "as" }), "a1")
        .await;
    assert_eq!(delegated.status(), 200, "{delegated:?}");
    socket.stop().await.expect("the socket stops");
    flush(&harness).await;

    assert_eq!(
        rows(
            &harness,
            "SELECT count(*)::text FROM audit_call WHERE function = 'keel.account.list'"
        )
        .await,
        ["0"]
    );
    let keyed = rows(
        &harness,
        &format!(
            "SELECT request_id || ' ' || coalesce(ip, '-') || ' ' || via || ' ' || principal_login || ' ' || coalesce(actor_login, '-') FROM audit_call WHERE {RECENT} AND function = 'auth.token.create' AND via = 'http'"
        ),
    )
    .await;
    assert_eq!(keyed, [format!("{request_id} 127.0.0.1 http a1 -")]);
    let framed = rows(
        &harness,
        &format!(
            "SELECT function || ' ' || coalesce(ip, '-') || ' ' || via || ' ' || principal_login || ' ' || coalesce(actor_login, '-') || ' ' || (request_id <> '')::text FROM audit_call WHERE {RECENT} AND via = 'socket' ORDER BY 1"
        ),
    )
    .await;
    assert_eq!(
        framed,
        [
            "auth.token.create - socket a1 owner true",
            "keel.account.create - socket owner - true",
        ]
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn event(at: DateTime<Utc>, principal: &Principal, n: u32) -> AuditEvent {
    AuditEvent {
        request_id: format!("req-flood-{n}"),
        function: "keel.account.create".into(),
        module: "keel".into(),
        mode: Mode::Write,
        principal: principal.clone(),
        client: Some(Client {
            ip: Some("10.0.0.7".parse().expect("an address")),
            user_agent: None,
            session: None,
        }),
        params: json!({ "kind": "user", "n": n }),
        subject: None,
        result: None,
        outcome: if n.is_multiple_of(2) {
            AuditOutcome::Committed
        } else {
            AuditOutcome::Failed {
                error: keel_testkit::CallError::new(keel_testkit::ErrorCode::Conflict),
            }
        },
        at,
        elapsed: Duration::from_millis(u64::from(n)),
    }
}

const FLOOD_COUNT: &str =
    "SELECT count(*)::text FROM audit_call WHERE request_id LIKE 'req-flood-%'";

#[tokio::test]
async fn flush_returns_after_the_batch_is_written_and_a_stop_leaves_no_event_unwritten() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let sink = harness
        .registry()
        .services()
        .get::<dyn AuditSink>()
        .expect("the audit module provides the sink");
    let a1 = harness.principal(Who::A1).clone();
    let at = harness.clock().now();
    let total = 25 * BATCH_ROWS as u32;
    for n in 0..total {
        sink.record(event(at, &a1, n));
    }
    flush(&harness).await;
    assert_eq!(rows(&harness, FLOOD_COUNT).await, [total.to_string()]);
    assert_eq!(
        rows(
            &harness,
            "SELECT status || ' ' || count(*)::text FROM audit_call WHERE request_id LIKE 'req-flood-%' GROUP BY status ORDER BY status"
        )
        .await,
        [
            format!("{STATUS_COMMITTED} {}", total / 2),
            format!("{STATUS_FAILED} {}", total / 2),
        ]
    );
    let listed = harness
        .call(
            Who::A1,
            "audit.list",
            json!({ "pageSize": 2, "filters": [{ "key": "status", "op": "eq", "value": STATUS_FAILED }] }),
        )
        .await;
    let page = listed
        .data()
        .unwrap_or_else(|| panic!("audit.list: {:?}", listed.error()));
    assert_eq!(page["total"], json!(total / 2));
    assert_eq!(page["data"].as_array().map(Vec::len), Some(2));

    for n in total..total + 3 {
        sink.record(event(at, &a1, n));
    }
    let flush = harness
        .registry()
        .services()
        .get::<Flush>()
        .expect("the audit module provides Flush");
    assert_eq!(flush.dropped(), 0);
    let database = harness.stop().await;
    assert_eq!(database.rows(FLOOD_COUNT).await, [(total + 3).to_string()]);
    let stopped = flush.flush().await.expect_err("the worker has stopped");
    assert_eq!(stopped.key(), "audit.error.not_running");
    database.finish().await;
}

struct CallFixture;

#[async_trait]
impl Fixture for CallFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let (_, principal) = harness
            .fixtures()
            .all()
            .find(|(who, principal)| {
                principal.account_id == account && *who != Who::Guest && *who != Who::Owner
            })
            .unwrap_or_else(|| panic!("no fixture principal lives in account {account}"));
        let request_id = format!("req-fixture-{}", Id::generate());
        let reply = harness
            .call_with(
                principal,
                "auth.token.create",
                json!({ "name": "fixture" }),
                CallOptions {
                    request_id: Some(request_id.clone()),
                    ..CallOptions::default()
                },
            )
            .await;
        assert!(reply.data().is_some(), "{:?}", reply.error());
        flush(harness).await;
        let ids = harness
            .database()
            .rows(&format!(
                "SELECT id FROM audit_call WHERE request_id = '{request_id}'"
            ))
            .await;
        ids.into_iter().next().expect("the fixture call left a row")
    }
}

async fn listed(harness: &Harness, who: Who, filters: Value, options: CallOptions) -> Vec<String> {
    let reply = harness
        .call_with(
            harness.principal(who),
            "audit.list",
            json!({ "filters": filters, "pageSize": 200 }),
            options,
        )
        .await;
    let page = reply
        .data()
        .unwrap_or_else(|| panic!("audit.list as {who}: {:?}", reply.error()));
    let mut rows: Vec<String> = page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            format!(
                "{} {}",
                row["function"].as_str().unwrap_or_default(),
                row["principalLogin"].as_str().unwrap_or_default()
            )
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn audit_list_filters_by_function_principal_status_request_and_time_within_the_callers_scope()
{
    let (harness, http) = boot().await;
    assert_eq!(
        harness.clock().now().to_rfc3339(),
        "2026-09-13T20:00:00+00:00"
    );
    let a2 = harness.principal(Who::A2).clone();
    for who in [Who::A1, Who::A2, Who::Unrelated] {
        let reply = harness
            .call_with(
                harness.principal(who),
                "auth.token.create",
                json!({ "name": who.login() }),
                CallOptions {
                    request_id: Some(format!("req-{who}")),
                    ..CallOptions::default()
                },
            )
            .await;
        assert!(reply.data().is_some(), "{who}: {:?}", reply.error());
    }
    let failed = harness
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "wide", "level": "admin" }),
        )
        .await;
    assert_eq!(failed.status(), 422, "{:?}", failed.envelope);
    let guest = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[JSON],
            Some(&json!({ "login": "a1", "password": "wrong-horse" }).to_string()),
        )
        .await;
    error_of(&guest, 401, "auth.error.credentials");
    flush(&harness).await;

    let none = json!([]);
    assert_eq!(
        listed(&harness, Who::A1, none.clone(), CallOptions::default()).await,
        ["auth.token.create a1", "auth.token.create a1"]
    );
    assert_eq!(
        listed(
            &harness,
            Who::Reseller,
            none.clone(),
            CallOptions::default()
        )
        .await,
        [
            "auth.token.create a1",
            "auth.token.create a1",
            "auth.token.create a2"
        ]
    );
    assert_eq!(
        listed(
            &harness,
            Who::Unrelated,
            none.clone(),
            CallOptions::default()
        )
        .await,
        ["auth.token.create unrelated"]
    );
    let everything = listed(
        &harness,
        Who::Admin,
        json!([{ "key": "at", "op": "gt", "value": SEEDED_AT }]),
        CallOptions::default(),
    )
    .await;
    assert_eq!(
        everything,
        [
            "auth.login guest",
            "auth.token.create a1",
            "auth.token.create a1",
            "auth.token.create a2",
            "auth.token.create unrelated",
        ]
    );
    assert!(
        listed(&harness, Who::Admin, none.clone(), CallOptions::default())
            .await
            .contains(&"keel.principal.create owner".to_owned())
    );
    assert_eq!(
        listed(
            &harness,
            Who::Admin,
            json!([{ "key": "function", "op": "eq", "value": "auth.login" }]),
            CallOptions::default()
        )
        .await,
        ["auth.login guest"]
    );
    assert_eq!(
        listed(
            &harness,
            Who::Admin,
            json!([{ "key": "principalId", "op": "eq", "value": a2.id }]),
            CallOptions::default()
        )
        .await,
        ["auth.token.create a2"]
    );
    assert_eq!(
        listed(
            &harness,
            Who::Reseller,
            json!([{ "key": "status", "op": "eq", "value": STATUS_FAILED }]),
            CallOptions::default()
        )
        .await,
        ["auth.token.create a1"]
    );
    assert_eq!(
        listed(
            &harness,
            Who::Admin,
            json!([{ "key": "requestId", "op": "eq", "value": "req-a2" }]),
            CallOptions::default()
        )
        .await,
        ["auth.token.create a2"]
    );
    let tokyo = CallOptions {
        tz: Some(keel_sdk::chrono_tz::Asia::Tokyo),
        ..CallOptions::default()
    };
    let since = json!([{ "key": "at", "op": "gte", "value": "2026-09-14" }]);
    assert_eq!(
        listed(&harness, Who::A1, since.clone(), tokyo.clone()).await,
        ["auth.token.create a1", "auth.token.create a1"]
    );
    assert!(
        listed(&harness, Who::A2, since, CallOptions::default())
            .await
            .is_empty()
    );
    let until = json!([{ "key": "at", "op": "lte", "value": "2026-09-13" }]);
    assert!(
        listed(&harness, Who::A1, until.clone(), tokyo)
            .await
            .is_empty()
    );
    assert_eq!(
        listed(&harness, Who::A2, until, CallOptions::default()).await,
        ["auth.token.create a2"]
    );

    let a2_rows = harness
        .database()
        .rows("SELECT id FROM audit_call WHERE principal_login = 'a2' AND function = 'auth.token.create'")
        .await;
    let got = harness
        .call(Who::A1, "audit.get", json!({ "id": a2_rows[0] }))
        .await;
    assert_eq!(got.status(), 404, "{:?}", got.envelope);
    let own = harness
        .database()
        .rows("SELECT id FROM audit_call WHERE principal_login = 'a1' AND status = 'committed'")
        .await;
    let got = harness
        .call(Who::A1, "audit.get", json!({ "id": own[0] }))
        .await;
    let row = got
        .data()
        .unwrap_or_else(|| panic!("audit.get: {:?}", got.error()));
    assert_eq!(row["principalLogin"], json!("a1"));
    assert_eq!(row["requestId"], json!("req-a1"));
    assert_eq!(row["paramsRedacted"], json!({ "name": "a1" }));
    assert_eq!(row["after"], json!(null));
    assert_eq!(row["status"], json!(STATUS_COMMITTED));

    harness
        .assert_reseller_sees_only_subtree("audit.list", &CallFixture)
        .await;
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_audit_descriptors_match_their_snapshots() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "audit", "testkit"]);
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let audit: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("audit."))
            .collect();
        if level < Level::User {
            assert!(audit.is_empty(), "{level}: {audit:?}");
        } else {
            let mut expected = FUNCTIONS.to_vec();
            expected.sort_unstable();
            assert_eq!(audit, expected, "{level}");
        }
    }
    let http = harness.http().await;
    let booted = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    let system = booted.json()["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .find(|group| group["id"] == json!("system"))
        .cloned()
        .expect("the system group");
    let node = system["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["func"] == json!("audit.list"))
        .cloned()
        .unwrap_or_else(|| panic!("no audit node in {system}"));
    assert_eq!(node["label"], json!("audit.list.title"));
    assert_eq!(MENU_PATH, "system/audit");
    let guest = http.get(Who::Guest, "/api/v1/boot").await;
    assert!(
        !guest.body.contains("audit.list"),
        "a guest sees the audit node: {}",
        guest.body
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
