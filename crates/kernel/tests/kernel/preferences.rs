use std::sync::Arc;

use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::system::{PreferenceSource, Preferences};
use keel_testkit::{Harness, Who, tokio};
use keel_types::DashboardLayout;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Seen {
    tz: String,
    lang: String,
    nested_tz: Option<String>,
}

struct ByLogin;

async fn login_of(principal: Id, db: &Db) -> Result<String> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT login FROM keel_principal WHERE id = $1",
            [principal.to_string().into()],
        ))
        .await?;
    Ok(row
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?
        .try_get("", "login")?)
}

#[async_trait]
impl PreferenceSource for ByLogin {
    async fn dashboard(&self, principal: Id, db: &Db) -> Result<Option<DashboardLayout>> {
        Ok(
            (login_of(principal, db).await? == "a1").then(|| DashboardLayout {
                order: vec!["zone.inner".into(), "zone.gone".into()],
                hidden: vec!["zone.read".into()],
            }),
        )
    }

    async fn preferences(&self, principal: Id, db: &Db) -> Result<Preferences> {
        let login = login_of(principal, db).await?;
        Ok(match login.as_str() {
            "a1" => Preferences {
                lang: Some("ru".into()),
                tz: Some("Asia/Tokyo".into()),
            },
            "a2" => Preferences {
                lang: Some("de".into()),
                tz: Some("Mars/Olympus".into()),
            },
            _ => Preferences::default(),
        })
    }
}

static ZONE: ModuleDef = ModuleDef {
    id: "zone",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: zone_init,
};

fn zone_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn PreferenceSource>(Arc::new(ByLogin))
        .function(
            function::<Nothing, Seen>("zone.read")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    let nested = call.invoke("zone.inner", json!({})).await?;
                    Ok(Seen {
                        tz: call.tz().name().to_owned(),
                        lang: call.principal().lang.clone(),
                        nested_tz: nested["tz"].as_str().map(str::to_owned),
                    })
                }),
        )
        .function(
            function::<Nothing, Seen>("zone.inner")
                .kind(Kind::Get)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    Ok(Seen {
                        tz: call.tz().name().to_owned(),
                        lang: call.principal().lang.clone(),
                        nested_tz: None,
                    })
                }),
        )
        .contribute(DashboardWidget::new(WidgetKind::Stat, "zone.read"))
        .contribute(DashboardWidget::new(WidgetKind::Stat, "zone.inner"));
    Ok(())
}

#[tokio::test]
async fn the_transport_overlays_the_preferred_language_and_hands_the_zone_to_every_call_of_the_request()
 {
    let harness = Harness::new().module(&ZONE).boot().await;
    let http = harness.http().await;
    let a1 = http.get(Who::A1, "/api/v1/call/zone.read").await;
    assert_eq!(a1.status, 200, "{}", a1.body);
    assert_eq!(
        a1.json()["data"],
        json!({ "tz": "Asia/Tokyo", "lang": "ru", "nested_tz": "Asia/Tokyo" })
    );
    assert_eq!(a1.json()["meta"]["principal"]["login"], json!("a1"));
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.json()["principal"]["tz"], json!("Asia/Tokyo"));
    assert_eq!(boot.json()["principal"]["lang"], json!("ru"));
    let catalogue = http.get(Who::A1, "/api/v1/i18n").await;
    assert_eq!(catalogue.json()["data"]["lang"], json!("ru"));

    let a2 = http.get(Who::A2, "/api/v1/call/zone.read").await;
    assert_eq!(a2.status, 200, "{}", a2.body);
    assert_eq!(
        a2.json()["data"],
        json!({ "tz": "UTC", "lang": "en", "nested_tz": "UTC" })
    );
    let boot = http.get(Who::A2, "/api/v1/boot").await;
    assert_eq!(boot.json()["principal"]["tz"], json!(null));
    assert_eq!(boot.json()["principal"]["lang"], json!("en"));

    let owner = http.get(Who::Owner, "/api/v1/call/zone.read").await;
    assert_eq!(owner.json()["data"]["tz"], json!("UTC"));
    http.stop().await.expect("the server stops");

    let direct = harness.call(Who::A1, "zone.read", json!({})).await;
    assert_eq!(
        direct.data(),
        Some(&json!({ "tz": "UTC", "lang": "en", "nested_tz": "UTC" })),
        "{:?}",
        direct.error()
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_boot_dashboard_follows_the_preferred_layout_and_flags_the_hidden_widgets() {
    let harness = Harness::new().module(&ZONE).boot().await;
    let http = harness.http().await;
    let a1 = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(a1.status, 200, "{}", a1.body);
    assert_eq!(
        a1.json()["dashboard"],
        json!([
            { "kind": "stat", "func": "zone.inner" },
            { "kind": "stat", "func": "zone.read", "hidden": true }
        ])
    );
    let a2 = http.get(Who::A2, "/api/v1/boot").await;
    assert_eq!(
        a2.json()["dashboard"],
        json!([
            { "kind": "stat", "func": "zone.read" },
            { "kind": "stat", "func": "zone.inner" }
        ])
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
