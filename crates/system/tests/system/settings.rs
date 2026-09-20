use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_system::auth::{
    PASSWORD_MIN_LENGTH, SETTING_LOCKOUT_MINUTES, SETTING_LOCKOUT_THRESHOLD,
    SETTING_PASSWORD_MIN_LENGTH, SETTING_RATE_GUEST_PER_MINUTE, SETTING_SESSION_ABSOLUTE_HOURS,
    SETTING_SESSION_IDLE_MINUTES,
};
use keel_system::{auth, settings};
use keel_testkit::{Answer, ErrorCode, Harness, Reply, TestHttp, Who, tokio};

const GREETING: &str = "probe.greeting";

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.0.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: probe_init,
};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Greeted {
    greeting: String,
}

fn probe_init(r: &mut Registrar) -> Result<()> {
    r.contribute(
        SettingDef::new(GREETING, schema_for!(String))
            .default("hello")
            .level(Level::Reseller),
    )
    .function(
        function::<Nothing, Greeted>("probe.greet")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Greeted {
                    greeting: call.setting(GREETING).await?,
                })
            }),
    );
    Ok(())
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> Harness {
    Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .boot()
        .await
}

fn refused(reply: &Reply, status: u16, code: ErrorCode, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), status, "{error:?}");
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(error.key, key, "{error:?}");
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

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn keys(page: &Value) -> Vec<&str> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["key"].as_str().expect("a key"))
        .collect()
}

async fn set(harness: &Harness, who: Who, key: &str, value: Value) -> Reply {
    harness
        .call(who, "settings.set", json!({ "key": key, "value": value }))
        .await
}

async fn login(http: &TestHttp, password: &str) -> Answer {
    http.request(
        "POST",
        "/api/v1/call/auth.login",
        &[("content-type", "application/json")],
        Some(&json!({ "login": "a1", "password": password }).to_string()),
    )
    .await
}

async fn greeting(harness: &Harness, who: Who) -> String {
    let reply = harness.call(who, "probe.greet", json!({})).await;
    data(&reply)["greeting"]
        .as_str()
        .expect("a greeting")
        .to_owned()
}

#[tokio::test]
async fn the_admin_lists_the_six_auth_definitions_and_a_raised_minimum_makes_the_password_refuse_twelve_characters()
 {
    let harness = boot().await;
    let listed = harness.call(Who::Admin, "settings.list", json!({})).await;
    let page = data(&listed);
    assert_eq!(
        keys(page),
        [
            SETTING_LOCKOUT_MINUTES,
            SETTING_LOCKOUT_THRESHOLD,
            SETTING_PASSWORD_MIN_LENGTH,
            SETTING_RATE_GUEST_PER_MINUTE,
            SETTING_SESSION_ABSOLUTE_HOURS,
            SETTING_SESSION_IDLE_MINUTES,
        ]
    );
    assert_eq!(page["total"], json!(6));
    let min_length = &page["data"][2];
    assert_eq!(
        min_length,
        &json!({
            "key": SETTING_PASSWORD_MIN_LENGTH,
            "module": "auth",
            "schema": { "type": "integer", "format": "uint32", "minimum": 0, "title": "uint32" },
            "default": PASSWORD_MIN_LENGTH,
            "level": "admin",
            "value": PASSWORD_MIN_LENGTH,
            "scope": null
        })
    );
    let a1 = harness.principal(Who::A1).id;
    let accepted = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": "twelve-chars" }),
        )
        .await;
    assert_eq!(accepted.status(), 200, "{:?}", accepted.error());

    let raised = set(&harness, Who::Admin, SETTING_PASSWORD_MIN_LENGTH, json!(16)).await;
    assert_eq!(
        data(&raised),
        &json!({
            "key": SETTING_PASSWORD_MIN_LENGTH,
            "module": "auth",
            "schema": { "type": "integer", "format": "uint32", "minimum": 0, "title": "uint32" },
            "default": PASSWORD_MIN_LENGTH,
            "level": "admin",
            "value": 16,
            "scope": Id::ROOT_ACCOUNT
        })
    );
    let short = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": "twelve-chars" }),
        )
        .await;
    field_error(&short, "password", "auth.error.password_short");
    assert_eq!(
        short.error().expect("an error").fields[0].params["min"],
        json!(16)
    );
    let long_enough = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": "sixteen-chars-ok" }),
        )
        .await;
    assert_eq!(long_enough.status(), 200, "{:?}", long_enough.error());
    let got = harness
        .call(
            Who::Owner,
            "settings.get",
            json!({ "key": SETTING_PASSWORD_MIN_LENGTH }),
        )
        .await;
    assert_eq!(data(&got)["value"], json!(16));
    assert_eq!(data(&got)["scope"], json!(Id::ROOT_ACCOUNT));
    let rows = harness
        .database()
        .rows("SELECT scope_id || ' ' || key || ' ' || value::text || ' ' || updated_by FROM settings_value")
        .await;
    assert_eq!(
        rows,
        [format!(
            "{} {SETTING_PASSWORD_MIN_LENGTH} 16 {}",
            Id::ROOT_ACCOUNT,
            harness.principal(Who::Admin).id
        )]
    );

    let above = set(
        &harness,
        Who::Reseller,
        SETTING_PASSWORD_MIN_LENGTH,
        json!(8),
    )
    .await;
    refused(&above, 403, ErrorCode::Forbidden, "settings.error.level");
    assert_eq!(
        above.error().expect("an error").params,
        json!({ "key": SETTING_PASSWORD_MIN_LENGTH, "level": "admin", "caller": "reseller" })
            .as_object()
            .cloned()
            .expect("params")
    );
    let unreadable = harness
        .call(
            Who::Reseller,
            "settings.get",
            json!({ "key": SETTING_PASSWORD_MIN_LENGTH }),
        )
        .await;
    refused(
        &unreadable,
        403,
        ErrorCode::Forbidden,
        "settings.error.level",
    );
    let reseller_list = harness
        .call(Who::Reseller, "settings.list", json!({}))
        .await;
    assert_eq!(data(&reseller_list)["total"], json!(0));
    let user_list = harness.call(Who::A1, "settings.list", json!({})).await;
    refused(&user_list, 403, ErrorCode::Forbidden, "keel.error.level");

    let wrong = set(
        &harness,
        Who::Admin,
        SETTING_PASSWORD_MIN_LENGTH,
        json!("many"),
    )
    .await;
    field_error(&wrong, "value", "keel.schema.type");
    let negative = set(&harness, Who::Admin, SETTING_PASSWORD_MIN_LENGTH, json!(-1)).await;
    field_error(&negative, "value", "keel.schema.minimum");
    let unknown = set(&harness, Who::Admin, "auth.password.max_length", json!(1)).await;
    refused(&unknown, 404, ErrorCode::NotFound, "keel.error.not_found");
    let unknown_get = harness
        .call(Who::Admin, "settings.get", json!({ "key": "nothing.here" }))
        .await;
    refused(
        &unknown_get,
        404,
        ErrorCode::NotFound,
        "keel.error.not_found",
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_value_inherits_from_the_root_down_the_account_path_and_a_composition_without_settings_resolves_the_default()
 {
    let harness = Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .module(&PROBE)
        .boot()
        .await;
    for who in [Who::Admin, Who::Reseller, Who::A1, Who::Unrelated] {
        assert_eq!(greeting(&harness, who).await, "hello", "{who}");
    }
    let listed = harness
        .call(Who::Reseller, "settings.list", json!({}))
        .await;
    assert_eq!(
        data(&listed)["data"],
        json!([{
            "key": GREETING,
            "module": "probe",
            "schema": { "type": "string", "title": "string" },
            "default": "hello",
            "level": "reseller",
            "value": "hello",
            "scope": null
        }])
    );

    let root = set(&harness, Who::Admin, GREETING, json!("root")).await;
    assert_eq!(data(&root)["scope"], json!(Id::ROOT_ACCOUNT));
    for who in [Who::Admin, Who::Reseller, Who::A1, Who::Unrelated] {
        assert_eq!(greeting(&harness, who).await, "root", "{who}");
    }

    let reseller_account = harness.fixtures().accounts.reseller;
    let own = set(&harness, Who::Reseller, GREETING, json!("reseller")).await;
    assert_eq!(data(&own)["value"], json!("reseller"));
    assert_eq!(data(&own)["scope"], json!(reseller_account));
    assert_eq!(greeting(&harness, Who::A1).await, "reseller");
    assert_eq!(greeting(&harness, Who::A2).await, "reseller");
    assert_eq!(greeting(&harness, Who::Reseller).await, "reseller");
    assert_eq!(greeting(&harness, Who::Unrelated).await, "root");
    assert_eq!(greeting(&harness, Who::Admin).await, "root");
    let admin_view = harness
        .call(Who::Admin, "settings.get", json!({ "key": GREETING }))
        .await;
    assert_eq!(data(&admin_view)["value"], json!("root"));
    assert_eq!(data(&admin_view)["scope"], json!(Id::ROOT_ACCOUNT));
    let reseller_view = harness
        .call(Who::Reseller, "settings.get", json!({ "key": GREETING }))
        .await;
    assert_eq!(data(&reseller_view)["value"], json!("reseller"));
    assert_eq!(data(&reseller_view)["scope"], json!(reseller_account));

    let searched = harness
        .call(
            Who::Admin,
            "settings.list",
            json!({ "search": "GREET", "sort": { "key": "module", "dir": "desc" } }),
        )
        .await;
    assert_eq!(keys(data(&searched)), [GREETING]);
    let filtered = harness
        .call(
            Who::Admin,
            "settings.list",
            json!({ "filters": [{ "key": "level", "value": ["admin"] }], "pageSize": 2, "page": 2 }),
        )
        .await;
    assert_eq!(data(&filtered)["total"], json!(6));
    assert_eq!(
        keys(data(&filtered)),
        [SETTING_PASSWORD_MIN_LENGTH, SETTING_RATE_GUEST_PER_MINUTE]
    );
    let ranged = harness
        .call(
            Who::Admin,
            "settings.list",
            json!({ "filters": [{ "key": "level", "op": "gt", "value": ["admin"] }] }),
        )
        .await;
    field_error(&ranged, "filters[0].op", "keel.error.list.list_value_op");
    harness.finish().await;

    let bare = Harness::new().module(&PROBE).boot().await;
    assert_eq!(greeting(&bare, Who::A1).await, "hello");
    assert_eq!(greeting(&bare, Who::Admin).await, "hello");
    let absent = bare.call(Who::Admin, "settings.list", json!({})).await;
    refused(
        &absent,
        404,
        ErrorCode::NotFound,
        "keel.error.function_not_found",
    );
    bare.finish().await;
}

#[tokio::test]
async fn the_settings_descriptors_match_their_snapshots() {
    let harness = boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let settings: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("settings."))
            .collect();
        let expected: &[&str] = if level < Level::User {
            &[]
        } else if level < Level::Reseller {
            &["settings.pref.get", "settings.pref.set"]
        } else if level < Level::Admin {
            &[
                "settings.get",
                "settings.list",
                "settings.pref.get",
                "settings.pref.set",
                "settings.set",
            ]
        } else {
            &[
                "settings.brand.delete",
                "settings.brand.get",
                "settings.brand.list",
                "settings.brand.logo",
                "settings.brand.set",
                "settings.get",
                "settings.list",
                "settings.pref.get",
                "settings.pref.set",
                "settings.set",
            ]
        };
        assert_eq!(settings, expected, "{level}");
    }
    harness.finish().await;
}

#[tokio::test]
async fn auth_reads_its_session_lifetime_lockout_threshold_and_guest_rate_from_the_stored_settings()
{
    let harness = Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .clock("2026-09-13T10:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    let a1 = harness.principal(Who::A1).id;
    let set_password = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": "correct-horse" }),
        )
        .await;
    assert_eq!(set_password.status(), 200, "{:?}", set_password.error());
    for (key, value) in [
        (SETTING_SESSION_IDLE_MINUTES, 5),
        (SETTING_SESSION_ABSOLUTE_HOURS, 1),
        (SETTING_LOCKOUT_THRESHOLD, 2),
        (SETTING_RATE_GUEST_PER_MINUTE, 4),
    ] {
        let stored = set(&harness, Who::Admin, key, json!(value)).await;
        assert_eq!(stored.status(), 200, "{key}: {:?}", stored.error());
    }
    let http = harness.http().await;
    let opened = login(&http, "correct-horse").await;
    assert_eq!(opened.status, 200, "{}", opened.body);
    let cookie = opened
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie: {:?}", opened.headers));
    let bounds = harness
        .database()
        .rows("SELECT idle_until::text || ' ' || absolute_until::text FROM auth_session")
        .await;
    assert_eq!(bounds, ["2026-09-13 10:05:00+00 2026-09-13 11:00:00+00"]);

    harness
        .clock()
        .advance(keel_sdk::chrono::Duration::minutes(3));
    let touched = http
        .request("GET", "/api/v1/boot", &[("cookie", &cookie)], None)
        .await;
    assert_eq!(touched.status, 200, "{}", touched.body);
    let idle = harness
        .database()
        .rows("SELECT idle_until::text FROM auth_session")
        .await;
    assert_eq!(idle, ["2026-09-13 10:08:00+00"]);

    let first = login(&http, "wrong").await;
    assert_eq!(first.status, 401, "{}", first.body);
    let second = login(&http, "wrong").await;
    assert_eq!(second.status, 401, "{}", second.body);
    for _ in 0..2 {
        let locked = login(&http, "correct-horse").await;
        assert_eq!(locked.status, 403, "{}", locked.body);
        assert_eq!(locked.json()["error"]["key"], json!("auth.error.locked"));
    }
    let limited = login(&http, "correct-horse").await;
    assert_eq!(limited.status, 429, "{}", limited.body);
    assert_eq!(
        limited.json()["error"]["key"],
        json!("auth.error.rate_limited")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
