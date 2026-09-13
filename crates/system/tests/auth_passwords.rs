use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_system::auth::{HASH_PREFIX, MODULE};
use keel_testkit::{Answer, ErrorCode, Harness, TestHttp, Who, tokio};

const FIRST: &str = "first-password";
const SECOND: &str = "second-password";

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn set_password(http: &TestHttp, who: Who, params: Value) -> Answer {
    http.post(who, "/api/v1/call/auth.password.set", &params)
        .await
}

fn error_of(answer: &Answer) -> (ErrorCode, String) {
    let body = answer.json();
    assert_eq!(body["ok"], json!(false), "{body}");
    let code = serde_json::from_value(body["error"]["code"].clone()).expect("an error code");
    let key = body["error"]["key"].as_str().expect("a key").to_owned();
    (code, key)
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
async fn the_ledger_and_the_tables_exist_and_a_set_password_is_stored_as_argon2id() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "auth", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_auth")
        .await;
    assert_eq!(applied, ["8"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'auth\\_%' ORDER BY table_name")
        .await;
    assert_eq!(
        tables,
        [
            "auth_login_challenge",
            "auth_login_key",
            "auth_password",
            "auth_recovery_code",
            "auth_session",
            "auth_token",
            "auth_totp"
        ]
    );
    let socket = harness.socket().await;
    let a1 = harness.principal(Who::A1).id;
    let set = socket
        .call(
            "auth.password.set",
            json!({ "principal": a1, "password": FIRST }),
        )
        .await;
    assert_eq!(
        serde_json::to_value(&set).expect("an envelope")["data"]["principal"],
        json!(a1.to_string())
    );
    let hashes = harness
        .database()
        .rows("SELECT hash FROM auth_password")
        .await;
    assert_eq!(hashes.len(), 1);
    assert!(hashes[0].starts_with(HASH_PREFIX), "{}", hashes[0]);
    assert!(!hashes[0].contains(FIRST));
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_principal_changes_its_own_password_with_the_current_one_and_an_admin_sets_it_without() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let http = harness.http().await;
    let a1 = harness.principal(Who::A1).id;
    let without_row =
        set_password(&http, Who::A1, json!({ "current": "x", "password": FIRST })).await;
    assert_eq!(without_row.status, 401, "{}", without_row.body);
    assert_eq!(
        error_of(&without_row),
        (ErrorCode::Unauthenticated, "auth.error.credentials".into())
    );
    let by_admin = set_password(
        &http,
        Who::Admin,
        json!({ "principal": a1, "password": FIRST }),
    )
    .await;
    assert_eq!(by_admin.status, 200, "{}", by_admin.body);
    assert_eq!(by_admin.json()["data"]["principal"], json!(a1.to_string()));
    let no_current = set_password(&http, Who::A1, json!({ "password": SECOND })).await;
    assert_eq!(no_current.status, 422, "{}", no_current.body);
    let body = no_current.json();
    assert_eq!(body["error"]["code"], json!("validation"));
    assert_eq!(body["error"]["fields"][0]["path"], json!("current"));
    assert_eq!(
        body["error"]["fields"][0]["key"],
        json!("auth.error.current_required")
    );
    let wrong = set_password(
        &http,
        Who::A1,
        json!({ "current": "not-it", "password": SECOND }),
    )
    .await;
    assert_eq!(wrong.status, 401, "{}", wrong.body);
    assert_eq!(
        error_of(&wrong),
        (ErrorCode::Unauthenticated, "auth.error.credentials".into())
    );
    let changed = set_password(
        &http,
        Who::A1,
        json!({ "current": FIRST, "password": SECOND }),
    )
    .await;
    assert_eq!(changed.status, 200, "{}", changed.body);
    let old_current = set_password(
        &http,
        Who::A1,
        json!({ "current": FIRST, "password": "third" }),
    )
    .await;
    assert_eq!(old_current.status, 401, "{}", old_current.body);
    let new_current = set_password(
        &http,
        Who::A1,
        json!({ "current": SECOND, "password": "third" }),
    )
    .await;
    assert_eq!(new_current.status, 200, "{}", new_current.body);
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_password")
        .await;
    assert_eq!(rows, ["1"]);
    let by_user = set_password(
        &http,
        Who::A2,
        json!({ "principal": a1, "password": "hijack" }),
    )
    .await;
    assert_eq!(by_user.status, 403, "{}", by_user.body);
    assert_eq!(by_user.json()["error"]["code"], json!("forbidden_field"));
    assert_eq!(
        by_user.json()["error"]["fields"][0]["path"],
        json!("principal")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_password_is_set_for_a_target_below_the_caller_inside_its_scope_only() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let http = harness.http().await;
    let accounts = harness.fixtures().accounts.clone();
    let second_reseller =
        create_principal(&harness, "reseller2", Level::Reseller, accounts.reseller).await;
    let second_admin = create_principal(&harness, "admin2", Level::Admin, accounts.root).await;
    let a1 = harness.principal(Who::A1).id;
    let reseller = harness.principal(Who::Reseller).id;
    let admin = harness.principal(Who::Admin).id;
    let unrelated = harness.principal(Who::Unrelated).id;

    let same_level = set_password(
        &http,
        Who::Reseller,
        json!({ "principal": second_reseller, "password": FIRST }),
    )
    .await;
    assert_eq!(same_level.status, 403, "{}", same_level.body);
    assert_eq!(
        error_of(&same_level),
        (ErrorCode::Forbidden, "auth.error.not_below".into())
    );
    let admin_peer = set_password(
        &http,
        Who::Admin,
        json!({ "principal": second_admin, "password": FIRST }),
    )
    .await;
    assert_eq!(admin_peer.status, 403, "{}", admin_peer.body);
    assert_eq!(
        error_of(&admin_peer),
        (ErrorCode::Forbidden, "auth.error.not_below".into())
    );
    for target in [unrelated, admin] {
        let foreign = set_password(
            &http,
            Who::Reseller,
            json!({ "principal": target, "password": FIRST }),
        )
        .await;
        assert_eq!(foreign.status, 404, "{target}: {}", foreign.body);
        assert_eq!(foreign.json()["error"]["code"], json!("not_found"));
    }
    let below = set_password(
        &http,
        Who::Reseller,
        json!({ "principal": a1, "password": FIRST }),
    )
    .await;
    assert_eq!(below.status, 200, "{}", below.body);
    let by_admin = set_password(
        &http,
        Who::Admin,
        json!({ "principal": reseller, "password": FIRST }),
    )
    .await;
    assert_eq!(by_admin.status, 200, "{}", by_admin.body);
    let own = set_password(
        &http,
        Who::Reseller,
        json!({ "current": FIRST, "password": SECOND }),
    )
    .await;
    assert_eq!(own.status, 200, "{}", own.body);
    let principals = harness
        .database()
        .rows("SELECT principal_id FROM auth_password ORDER BY principal_id")
        .await;
    let mut expected = vec![a1.to_string(), reseller.to_string()];
    expected.sort();
    assert_eq!(principals, expected);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_password_descriptor_matches_its_snapshots_and_marks_both_passwords_secret() {
    let harness = Harness::new().module(&MODULE).boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let expected: &[&str] = match level {
            Level::Guest => &["auth.key.redeem", "auth.login", "auth.totp.verify"],
            Level::Service => &[
                "auth.key.redeem",
                "auth.login",
                "auth.logout",
                "auth.su.exit",
                "auth.totp.verify",
            ],
            Level::User => &[
                "auth.key.redeem",
                "auth.login",
                "auth.logout",
                "auth.password.set",
                "auth.su.exit",
                "auth.token.create",
                "auth.token.list",
                "auth.token.revoke",
                "auth.totp.confirm",
                "auth.totp.disable",
                "auth.totp.enroll",
                "auth.totp.verify",
            ],
            _ => &[
                "auth.key.issue",
                "auth.key.redeem",
                "auth.login",
                "auth.logout",
                "auth.password.set",
                "auth.su",
                "auth.su.exit",
                "auth.token.create",
                "auth.token.list",
                "auth.token.revoke",
                "auth.totp.confirm",
                "auth.totp.disable",
                "auth.totp.enroll",
                "auth.totp.verify",
            ],
        };
        assert_eq!(names, expected, "{level}");
    }
    let function = harness
        .registry()
        .function("auth.password.set")
        .expect("the function is registered");
    let params = serde_json::to_value(&function.descriptor().params).expect("a schema");
    for field in ["password", "current"] {
        assert_eq!(
            params["properties"][field]["x-keel"]["secret"],
            json!(true),
            "{field}"
        );
        assert_eq!(
            params["properties"][field]["x-ui"]["control"],
            json!("password"),
            "{field}"
        );
    }
    assert_eq!(
        params["properties"]["principal"]["x-keel"]["level"],
        json!("reseller")
    );
    harness.finish().await;
}
