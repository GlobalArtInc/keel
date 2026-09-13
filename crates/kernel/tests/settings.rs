use std::sync::Arc;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_sdk::system::AccountSource;
use keel_testkit::{ErrorCode, Harness, Who, tokio};

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Answered {
    ttl: u32,
    name: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Chain {
    ancestry: Vec<Id>,
}

static DEFAULTS: ModuleDef = ModuleDef {
    id: "defaults",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: defaults_init,
};

fn defaults_init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn SettingsSource>()
        .contribute(SettingDef::new("defaults.ttl", schema_for!(u32)).default(3600))
        .contribute(SettingDef::new("defaults.name", schema_for!(String)))
        .function(
            function::<Nothing, Answered>("defaults.read")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    Ok(Answered {
                        ttl: call.setting("defaults.ttl").await?,
                        name: call.setting("defaults.name").await?,
                    })
                }),
        )
        .function(
            function::<Nothing, Nothing>("defaults.strict")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    let _: String = call.setting("defaults.name").await?;
                    Ok(Nothing {})
                }),
        )
        .function(
            function::<Nothing, Chain>("defaults.chain")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    let accounts = call.require::<dyn AccountSource>()?;
                    Ok(Chain {
                        ancestry: accounts
                            .ancestry(call.principal().account_id, call.db())
                            .await?,
                    })
                }),
        );
    Ok(())
}

struct Fixed;

#[async_trait]
impl SettingsSource for Fixed {
    async fn resolve(&self, key: &str, _: Id, _: &Db) -> Result<Option<Value>> {
        Ok((key == "defaults.ttl").then(|| json!(60)))
    }
}

static FIXED: ModuleDef = ModuleDef {
    id: "fixed",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: fixed_init,
};

fn fixed_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn SettingsSource>(Arc::new(Fixed));
    Ok(())
}

#[tokio::test]
async fn without_a_settings_module_the_platform_answers_the_contributed_defaults_and_a_provided_source_replaces_it()
 {
    let harness = Harness::new().module(&DEFAULTS).boot().await;
    let read = harness.call(Who::A1, "defaults.read", json!({})).await;
    assert_eq!(
        read.data(),
        Some(&json!({ "ttl": 3600, "name": null })),
        "{:?}",
        read.error()
    );
    let strict = harness.call(Who::A1, "defaults.strict", json!({})).await;
    let error = strict.error().expect("a setting without a value");
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.key, "keel.error.setting_value");
    assert_eq!(error.params["key"], json!("defaults.name"));
    harness.finish().await;

    let provided = Harness::new().module(&DEFAULTS).module(&FIXED).boot().await;
    let read = provided.call(Who::A1, "defaults.read", json!({})).await;
    assert_eq!(
        read.data(),
        Some(&json!({ "ttl": 60, "name": null })),
        "{:?}",
        read.error()
    );
    provided.finish().await;
}

#[tokio::test]
async fn the_platform_answers_the_ancestry_of_an_account_from_itself_up_to_the_root() {
    let harness = Harness::new().module(&DEFAULTS).boot().await;
    let accounts = harness.fixtures().accounts.clone();
    for (who, expected) in [
        (
            Who::A1,
            json!([accounts.a1, accounts.reseller, accounts.root]),
        ),
        (Who::Reseller, json!([accounts.reseller, accounts.root])),
        (Who::Unrelated, json!([accounts.unrelated, accounts.root])),
        (Who::Admin, json!([accounts.root])),
    ] {
        let reply = harness.call(who, "defaults.chain", json!({})).await;
        let data = reply
            .data()
            .unwrap_or_else(|| panic!("{who}: {:?}", reply.error()));
        assert_eq!(data["ancestry"], expected, "{who}");
    }
    harness.finish().await;
}
