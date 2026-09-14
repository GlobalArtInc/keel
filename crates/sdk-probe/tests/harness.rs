use std::path::{Path, PathBuf};

use keel_sdk::prelude::*;
use keel_sdk_probe::{MODULE, ProbeConfig, SECTION};
use keel_testkit::{ErrorCode, Fixture, Harness, Who, tokio};

static MANIFEST: ProductManifest = ProductManifest {
    id: "probe",
    name: "product.probe.brand",
    version: "0.1.0",
    api: 1,
    brand: Brand {
        logo: "product.probe.logo",
        hue: 200,
        chroma: 0.1,
    },
    modules: &[&MODULE],
    menu: &[MenuGroup {
        id: "probe",
        label: "menu.probe",
        icon: "notebook",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[],
    i18n: &[("en", r#"{ "product": { "probe": { "brand": "Probe" } } }"#)],
    assets: &[],
};

struct ItemFixture;

#[async_trait]
impl Fixture for ItemFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                "probe.item.edit",
                json!({ "title": format!("item of {account}"), "account": account }),
            )
            .await;
        let data = reply
            .data()
            .unwrap_or_else(|| panic!("probe.item.edit failed: {:?}", reply.error()));
        data["id"].as_str().expect("an id").to_owned()
    }
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn titles(page: &Value) -> Vec<&str> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().expect("a title"))
        .collect()
}

#[tokio::test]
async fn the_probe_bundle_boots_on_the_harness_and_scopes_its_items_by_account() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    assert_eq!(harness.registry().product_ids(), ["probe"]);
    assert_eq!(harness.modules(), ["keel", "probe", "testkit"]);

    let saved = harness
        .call_as(Level::User, "probe.item.edit", json!({ "title": "mine" }))
        .await;
    let id = saved.data().expect("the item is saved")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let mine = harness
        .call_as(Level::User, "probe.item.list", json!({}))
        .await;
    assert_eq!(titles(mine.data().expect("a page")), ["mine"]);
    let a2 = harness.call(Who::A2, "probe.item.list", json!({})).await;
    assert!(titles(a2.data().expect("a page")).is_empty());
    let reseller = harness
        .call_as(Level::Reseller, "probe.item.list", json!({}))
        .await;
    assert_eq!(titles(reseller.data().expect("a page")), ["mine"]);
    let foreign = harness
        .call(
            Who::Unrelated,
            "probe.item.edit",
            json!({ "id": id, "title": "taken" }),
        )
        .await;
    assert_eq!(
        foreign.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    let account_by_user = harness
        .call_as(
            Level::User,
            "probe.item.edit",
            json!({ "title": "x", "account": harness.fixtures().accounts.a2 }),
        )
        .await;
    assert_eq!(
        account_by_user.error().map(|error| error.code),
        Some(ErrorCode::ForbiddenField)
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_invariants_hold_for_the_probe_items() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    harness
        .assert_reseller_sees_only_subtree("probe.item.list", &ItemFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row("probe.item.edit", &ItemFixture)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn the_descriptors_of_the_probe_match_their_snapshots_per_level() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let expected: &[&str] = if level < Level::User {
            &[]
        } else {
            &["probe.config", "probe.item.edit", "probe.item.list"]
        };
        assert_eq!(names, expected, "{level}");
    }
    harness.finish().await;
}

#[tokio::test]
async fn the_probe_answers_over_http_for_the_principal_named_in_the_test_header() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let server = harness.http().await;
    let saved = server
        .post(
            Who::A1,
            "/api/v1/call/probe.item.edit",
            &json!({ "title": "over http" }),
        )
        .await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let listed = server
        .get(Who::Reseller, "/api/v1/call/probe.item.list")
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert_eq!(titles(&listed.json()["data"]), ["over http"]);
    let boot = server.get(Who::A1, "/api/v1/boot").await;
    let boot = boot.json();
    assert_eq!(boot["products"][0]["id"], json!("probe"));
    assert_eq!(boot["brand"]["name"], json!("product.probe.brand"));
    assert_eq!(boot["menu"][0]["id"], json!("probe"));
    server.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_valid_section_reaches_start_and_the_handlers_an_absent_one_yields_the_defaults() {
    let configured = Harness::new()
        .product(&MANIFEST)
        .section(SECTION, json!({ "greeting": "from the file", "limit": 42 }))
        .section("unclaimed", json!({ "anything": true }))
        .boot()
        .await;
    let expected = ProbeConfig {
        greeting: "from the file".into(),
        limit: 42,
    };
    assert!(
        keel_sdk_probe::started_with().contains(&expected),
        "{:?}",
        keel_sdk_probe::started_with()
    );
    let read = configured
        .call_as(Level::User, "probe.config", json!({}))
        .await;
    assert_eq!(
        read.data(),
        Some(&json!({ "greeting": "from the file", "limit": 42 })),
        "{:?}",
        read.error()
    );
    configured.finish().await;

    let bare = Harness::new().product(&MANIFEST).boot().await;
    let read = bare.call_as(Level::User, "probe.config", json!({})).await;
    assert_eq!(
        read.data(),
        Some(&json!({ "greeting": "hello", "limit": 10 })),
        "{:?}",
        read.error()
    );
    assert!(keel_sdk_probe::started_with().contains(&ProbeConfig::default()));
    bare.finish().await;
}

#[tokio::test]
async fn a_section_of_the_wrong_shape_fails_the_boot_naming_the_module_the_section_and_the_path() {
    for (section, path, fragment) in [
        (
            json!({ "greeting": "x", "limit": "many" }),
            "probe.limit",
            "many",
        ),
        (json!({ "greeting": "x", "limit": 0 }), "probe.limit", "0"),
        (json!({ "greting": "x" }), "probe", "greting"),
        (json!("just a string"), "probe", "just a string"),
    ] {
        let error = Harness::new()
            .product(&MANIFEST)
            .section(SECTION, section.clone())
            .try_boot()
            .await
            .err()
            .unwrap_or_else(|| panic!("{section} must not boot"))
            .to_string();
        assert!(error.contains("module probe"), "{section}: {error}");
        assert!(error.contains("config.yaml"), "{section}: {error}");
        assert!(
            error.contains(&format!("does not fit at {path}:")),
            "{section}: {error}"
        );
        assert!(error.contains(fragment), "{section}: {error}");
    }
}
