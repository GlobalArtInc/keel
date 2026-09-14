use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_system::settings;
use keel_testkit::{Harness, Who, tokio};

const GREETING: &str = "probe.greeting";
const CALL: &str = "/api/v1/call";

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.0.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: probe_init,
};

fn probe_init(r: &mut Registrar) -> Result<()> {
    r.contribute(
        SettingDef::new(GREETING, schema_for!(String))
            .default("hello")
            .level(Level::Admin),
    );
    Ok(())
}

#[tokio::test]
async fn settings_alone_boots_with_its_tables_and_the_admin_lists_and_sets_a_value_over_http() {
    let harness = Harness::new()
        .module(&settings::MODULE)
        .module(&PROBE)
        .boot()
        .await;
    assert_eq!(harness.modules(), ["keel", "probe", "settings", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_settings")
        .await;
    assert_eq!(applied, ["3"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'settings\\_%' ORDER BY table_name")
        .await;
    assert_eq!(
        tables,
        ["settings_brand", "settings_pref", "settings_value"]
    );

    let http = harness.http().await;
    let listed = http
        .post(Who::Admin, &format!("{CALL}/settings.list"), &json!({}))
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let page = &listed.json()["data"];
    assert_eq!(page["total"], json!(1), "{}", listed.body);
    assert_eq!(page["data"][0]["key"], json!(GREETING));
    assert_eq!(page["data"][0]["value"], json!("hello"));
    assert_eq!(page["data"][0]["scope"], Value::Null);

    let set = http
        .post(
            Who::Admin,
            &format!("{CALL}/settings.set"),
            &json!({ "key": GREETING, "value": "hi" }),
        )
        .await;
    assert_eq!(set.status, 200, "{}", set.body);
    assert_eq!(set.json()["data"]["value"], json!("hi"), "{}", set.body);
    let stored = harness
        .database()
        .rows("SELECT key || ' ' || value::text FROM settings_value")
        .await;
    assert_eq!(stored, [format!("{GREETING} \"hi\"")]);

    let refused = http
        .post(Who::A1, &format!("{CALL}/settings.list"), &json!({}))
        .await;
    assert_eq!(refused.status, 403, "{}", refused.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
