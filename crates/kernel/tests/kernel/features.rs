use std::sync::Arc;

use keel_kernel::Composition;
use keel_kernel::features;
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::Features;
use keel_testkit::{Harness, Who, tokio};
use keel_types::{ActionRole, Boot, ErrorCode, Scope};
use serde_json::json;

const EN: &str = r#"{
    "orbit": { "chart": { "list": { "title": "Charts" },
                          "action": { "report": "Report", "touch": "Touch" },
                          "column": { "stars": "Stars" } },
               "report": { "title": "Reports" },
               "sky": { "title": "Sky" }, "deck": { "title": "Deck" } },
    "menu": { "orbit": "Orbit" }
}"#;
const ORBIT_EN: &str = r#"{ "product": { "orbit": { "brand": "Orbit" } } }"#;
const COMET_EN: &str = r#"{ "product": { "comet": { "brand": "Comet" } } }"#;

const CHARTS: &str = "orbit.charts";
const EXPORT: &str = "orbit.export";
const REPORTS: &str = "orbit.reports";
const TELESCOPE: &str = "orbit.telescope";
const REPORT_READ: &str = "orbit.report.read";

#[derive(Deserialize, Serialize, JsonSchema)]
struct Chart {
    id: Id,
    title: String,
    stars: u32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

static ORBIT_MODULE: ModuleDef = ModuleDef {
    id: "orbit",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: orbit_init,
};

fn orbit_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<ListQuery, Page<Chart>>("orbit.chart.list")
            .kind(Kind::List)
            .level(Level::User)
            .feature(CHARTS)
            .unowned()
            .ui(List::new("orbit.chart.list.title")
                .column(Column::new("title", ColumnType::Text))
                .column(Column::new("stars", ColumnType::Number).edit("orbit.chart.rename"))
                .action(Action::new("report", ActionRole::Button).function("orbit.report"))
                .action(Action::new("touch", ActionRole::Button).function("orbit.chart.touch"))
                .row_actions(["report", "touch"]))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .function(
        function::<Nothing, Nothing>("orbit.chart.touch")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .feature(EXPORT)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
    )
    .function(
        function::<Nothing, Nothing>("orbit.chart.rename")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .feature(REPORTS)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
    )
    .function(
        function::<ListQuery, Page<Chart>>("orbit.report")
            .kind(Kind::List)
            .access(Level::User, REPORT_READ)
            .feature(REPORTS)
            .unowned()
            .ui(List::new("orbit.report.title").column(Column::new("title", ColumnType::Text)))
            .handler(|_: Call<ListQuery>| async { Ok(Page::new(Vec::new(), 0)) }),
    )
    .permission(REPORT_READ, &[Level::User, Level::Reseller])
    .view(
        "orbit.sky",
        ViewSpec::new(Level::User)
            .menu(
                MenuNode::new("orbit/sky", "orbit.sky")
                    .label("orbit.sky.title")
                    .order(30),
            )
            .needs([TELESCOPE]),
    )
    .view(
        "orbit.deck",
        ViewSpec::new(Level::User)
            .menu(
                MenuNode::new("orbit/deck", "orbit.deck")
                    .label("orbit.deck.title")
                    .order(40),
            )
            .needs([CHARTS]),
    )
    .menu(MenuNode::new("orbit/charts", "orbit.chart.list").order(10))
    .menu(MenuNode::new("orbit/reports", "orbit.report").order(20))
    .i18n("en", EN);
    Ok(())
}

static ORBIT: ProductManifest = ProductManifest {
    id: "orbit",
    name: "product.orbit.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.orbit.logo",
        hue: 200,
        chroma: 0.1,
    },
    modules: &[&ORBIT_MODULE],
    menu: &[MenuGroup {
        id: "orbit",
        label: "menu.orbit",
        icon: "globe",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[
        Edition {
            id: "lite",
            features: &[CHARTS],
        },
        Edition {
            id: "pro",
            features: &[CHARTS, EXPORT],
        },
    ],
    i18n: &[("en", ORBIT_EN)],
    assets: &[],
};

static COMET: ProductManifest = ProductManifest {
    id: "comet",
    name: "product.comet.brand",
    version: "0.1.0",
    api: keel_types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.comet.logo",
        hue: 20,
        chroma: 0.1,
    },
    modules: &[],
    menu: &[],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[Edition {
        id: "lite",
        features: &["comet.trails", CHARTS],
    }],
    i18n: &[("en", COMET_EN)],
    assets: &[],
};

struct Leased(Vec<String>);

impl Features for Leased {
    fn enabled(&self) -> Vec<String> {
        self.0.clone()
    }

    fn allows(&self, feature: &str) -> bool {
        self.0.iter().any(|known| known == feature)
    }
}

static LEASE_MODULE: ModuleDef = ModuleDef {
    id: "lease",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: lease_init,
};

fn lease_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn Features>(Arc::new(Leased(vec![REPORTS.to_owned()])));
    Ok(())
}

fn enabled_features(composition: Composition) -> Vec<String> {
    let frozen = composition
        .resolve()
        .expect("the composition resolves")
        .init()
        .expect("the modules register")
        .freeze()
        .expect("the registries freeze");
    frozen
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("the kernel provides the feature set")
        .enabled()
}

fn unlicensed(reply: &keel_kernel::Reply) {
    let error = reply.error().expect("the call is refused");
    assert_eq!(error.code, ErrorCode::License, "{error:?}");
    assert_eq!(reply.status(), 402);
    assert_eq!(error.key, "keel.error.license");
    assert_eq!(error.params["feature"], REPORTS);
}

fn names(listing: &Value) -> Vec<&str> {
    listing
        .as_array()
        .expect("a descriptor array")
        .iter()
        .filter_map(|descriptor| descriptor["name"].as_str())
        .filter(|name| name.starts_with("orbit."))
        .collect()
}

#[tokio::test]
async fn a_function_of_a_feature_not_in_the_set_answers_license_and_is_cut_from_the_projection_and_the_menu()
 {
    let harness = Harness::new().product(&ORBIT).boot().await;

    unlicensed(&harness.call(Who::A1, "orbit.report", json!({})).await);
    unlicensed(&harness.call(Who::Admin, "orbit.report", json!({})).await);
    let touched = harness.call(Who::A1, "orbit.chart.touch", json!({})).await;
    assert_eq!(touched.data(), Some(&json!({})), "{:?}", touched.error());

    let guest = harness.call(Who::Guest, "orbit.report", json!({})).await;
    assert_eq!(guest.error().unwrap().key, "keel.error.unauthenticated");
    let mut revoked = harness.principal(Who::A1).clone();
    revoked.grants.insert(REPORT_READ.to_owned(), false);
    let revoked = harness
        .call_as_principal(&revoked, "orbit.report", json!({}))
        .await;
    assert_eq!(revoked.error().unwrap().key, "keel.error.permission");
    let mut scoped = harness.principal(Who::A1).clone();
    scoped.scope = Scope::Functions(vec!["orbit.chart.list".to_owned()]);
    unlicensed(
        &harness
            .call_as_principal(&scoped, "orbit.report", json!({}))
            .await,
    );
    let outside = harness
        .call_as_principal(&scoped, "orbit.chart.touch", json!({}))
        .await;
    assert_eq!(outside.error().unwrap().key, "keel.error.token_scope");

    let http = harness.http().await;
    let listing = http.get(Who::A1, "/api/v1/functions").await;
    assert_eq!(listing.status, 200, "{}", listing.body);
    assert_eq!(
        names(&listing.json()),
        ["orbit.chart.list", "orbit.chart.touch"]
    );
    let admin = http.get(Who::Admin, "/api/v1/functions").await;
    assert_eq!(
        names(&admin.json()),
        ["orbit.chart.list", "orbit.chart.touch"]
    );
    let report = http.get(Who::A1, "/api/v1/functions/orbit.report").await;
    assert_eq!(report.status, 402, "{}", report.body);
    assert_eq!(report.json()["error"]["key"], json!("keel.error.license"));
    assert_eq!(report.json()["error"]["params"]["feature"], json!(REPORTS));

    let charts = http
        .get(Who::A1, "/api/v1/functions/orbit.chart.list")
        .await;
    assert_eq!(charts.status, 200, "{}", charts.body);
    let descriptor = charts.json();
    assert_eq!(descriptor["access"]["feature"], json!(CHARTS));
    let ui = &descriptor["ui"];
    let actions: Vec<&str> = ui["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .filter_map(|action| action["id"].as_str())
        .collect();
    assert_eq!(actions, ["touch"]);
    assert_eq!(ui["row_actions"], json!(["touch"]));
    let columns = ui["columns"].as_array().expect("columns");
    assert_eq!(columns.len(), 2);
    assert!(columns[1].get("edit").is_none(), "{}", columns[1]);

    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    let boot: Boot = serde_json::from_value(boot.json()).expect("a boot payload");
    assert_eq!(boot.features, [CHARTS, EXPORT]);
    assert_eq!(boot.views, ["orbit.deck"]);
    let orbit: Vec<&str> = boot
        .menu
        .iter()
        .filter(|group| group.id == "orbit")
        .flat_map(|group| group.items.iter())
        .map(|item| item.path.as_str())
        .collect();
    assert_eq!(orbit, ["orbit/charts", "orbit/deck"]);
    harness.finish().await;
}

#[test]
fn the_m4_feature_set_names_every_feature_of_every_enabled_product() {
    let orbit_alone = enabled_features(
        Composition::new()
            .product(&ORBIT)
            .product(&COMET)
            .enabled(["orbit"]),
    );
    assert_eq!(orbit_alone, [CHARTS, EXPORT]);
    assert_eq!(orbit_alone, features::every(&[&ORBIT]));

    let both = enabled_features(
        Composition::new()
            .product(&COMET)
            .product(&ORBIT)
            .enabled(["comet", "orbit"]),
    );
    assert_eq!(both, ["comet.trails", CHARTS, EXPORT]);
    assert_eq!(both, features::every(&[&COMET, &ORBIT]));

    let none = enabled_features(
        Composition::new()
            .product(&ORBIT)
            .enabled(Vec::<&str>::new()),
    );
    assert!(none.is_empty(), "{none:?}");

    let leased = enabled_features(
        Composition::new()
            .module(&LEASE_MODULE)
            .product(&ORBIT)
            .enabled(["orbit"]),
    );
    assert_eq!(leased, [REPORTS]);
}
