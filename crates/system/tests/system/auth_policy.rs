use std::sync::{Mutex, PoisonError};

use keel_sdk::prelude::*;
use keel_system::auth::{
    MODULE, PASSWORD_MIN_LENGTH, SETTING_LOCKOUT_MINUTES, SETTING_LOCKOUT_THRESHOLD,
    SETTING_PASSWORD_MIN_LENGTH, SETTING_RATE_GUEST_PER_MINUTE, SETTING_RESET_MINUTES,
    SETTING_SESSION_ABSOLUTE_HOURS, SETTING_SESSION_IDLE_MINUTES, SETTING_SHELL_URL,
};
use keel_testkit::{Harness, Reply, Who, tokio};

static SEEN: Mutex<Vec<(String, Option<Value>, Level)>> = Mutex::new(Vec::new());

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.0.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: probe_init,
};

fn probe_init(r: &mut Registrar) -> Result<()> {
    r.start(|ctx| async move {
        let seen: Vec<(String, Option<Value>, Level)> = ctx
            .contributions::<SettingDef>()
            .map(|def| {
                (
                    def.key().to_owned(),
                    def.default_value().cloned(),
                    def.level_value(),
                )
            })
            .collect();
        *SEEN.lock().unwrap_or_else(PoisonError::into_inner) = seen;
        Ok(())
    });
    Ok(())
}

fn field_error(reply: &Reply, path: &str, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), 422, "{error:?}");
    let field = error.fields.first().expect("a field error");
    assert_eq!(field.path, path, "{error:?}");
    assert_eq!(field.key.as_deref(), Some(key), "{error:?}");
}

#[tokio::test]
async fn the_policy_refuses_a_short_password_and_one_equal_to_the_login_wherever_it_is_set() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let a1 = harness.principal(Who::A1).id;
    let short = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": "eightchr" }),
        )
        .await;
    field_error(&short, "password", "auth.error.password_short");
    assert_eq!(
        short.error().expect("an error").fields[0].params["min"],
        json!(PASSWORD_MIN_LENGTH)
    );
    let created = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({
                "login": "long-login-user",
                "level": "user",
                "account": harness.principal(Who::A1).account_id
            }),
        )
        .await;
    let member: Id = created
        .data()
        .and_then(|data| data["id"].as_str())
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("keel.principal.create failed: {:?}", created.error()));
    for password in ["long-login-user", "Long-Login-User"] {
        let same = harness
            .call(
                Who::Admin,
                "auth.password.set",
                json!({ "principal": member, "password": password }),
            )
            .await;
        field_error(&same, "password", "auth.error.password_login");
    }
    let set = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": member, "password": "twelve-chars" }),
        )
        .await;
    assert_eq!(
        set.data(),
        Some(&json!({ "principal": member })),
        "{:?}",
        set.error()
    );
    let own_short = harness
        .call_as_principal(
            &Principal {
                login: "long-login-user".into(),
                id: member,
                ..harness.principal(Who::A1).clone()
            },
            "auth.password.set",
            json!({ "current": "twelve-chars", "password": "LONG-login-USER" }),
        )
        .await;
    field_error(&own_short, "password", "auth.error.password_login");
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM auth_password")
        .await;
    assert_eq!(rows, ["1"]);
    harness.finish().await;
}

#[tokio::test]
async fn a_module_reads_the_eight_setting_definitions_of_auth_with_their_defaults() {
    let harness = Harness::new().module(&MODULE).module(&PROBE).boot().await;
    let seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let expected: Vec<(String, Option<Value>, Level)> = [
        (SETTING_PASSWORD_MIN_LENGTH, 12),
        (SETTING_SESSION_IDLE_MINUTES, 30),
        (SETTING_SESSION_ABSOLUTE_HOURS, 12),
        (SETTING_LOCKOUT_THRESHOLD, 5),
        (SETTING_LOCKOUT_MINUTES, 15),
        (SETTING_RATE_GUEST_PER_MINUTE, 30),
        (SETTING_RESET_MINUTES, 60),
    ]
    .into_iter()
    .map(|(key, default)| (key.to_owned(), Some(json!(default)), Level::Admin))
    .chain([(SETTING_SHELL_URL.to_owned(), Some(json!("")), Level::Admin)])
    .collect();
    assert_eq!(seen, expected);
    harness.finish().await;
}
