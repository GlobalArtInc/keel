use std::sync::Arc;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::system::Features;
use keel::types::ErrorCode;
use keel_system::license::INSTALL;
use keel_testkit::license::Issuer;
use keel_testkit::{Harness, Reply, Who, tokio};

use crate::support::data;

const LICENSE: &str = "keel.error.license";
const ENABLE: &str = "keel.product.enable";
const PATIENCE: Duration = Duration::from_secs(10);

static DNS: ProductManifest = ProductManifest {
    id: "dns",
    name: "product.dns.brand",
    version: "0.1.0",
    api: keel::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.fixture.logo",
        hue: 120,
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
        features: &[],
    }],
    i18n: &[("en", r#"{ "product": { "dns": { "brand": "DNS" } } }"#)],
    assets: &[],
};

fn features(harness: &Harness) -> Arc<dyn Features> {
    harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set")
}

fn refused_by_license(reply: &Reply) -> &keel::sdk::serde_json::Map<String, Value> {
    let error = reply.error().expect("the license refuses the call");
    assert_eq!(error.code, ErrorCode::License, "{error:?}");
    assert_eq!(reply.status(), 402);
    assert_eq!(error.key, LICENSE);
    &error.params
}

#[tokio::test]
async fn a_lease_that_lists_dns_alone_refuses_keel_product_enable_panel() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = Harness::new()
        .product(&DNS)
        .product(&keel_panel::MANIFEST)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::license::MODULE)
        .license_key(issuer.key())
        .enabled(["dns"])
        .boot()
        .await;
    let features = features(&harness);
    assert!(features.may_enable("panel"));
    let now = harness.clock().now();
    let signed = issuer
        .sign(&json!({
            "instance_id": harness.instance(),
            "issued_at": now,
            "expires_at": now + keel::sdk::chrono::Days::new(365),
            "grace_days": 30,
            "products": [{ "id": "dns", "edition": "lite" }],
        }))
        .expect("the lease is signed");
    data(
        &harness
            .call(Who::Admin, INSTALL, json!({ "lease": signed }))
            .await,
    );
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while features.may_enable("panel") || !features.may_enable("dns") {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease did not settle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let refused = harness
        .call(Who::Admin, ENABLE, json!({ "id": "panel" }))
        .await;
    assert_eq!(refused_by_license(&refused)["product"], json!("panel"));
    let dns = harness
        .call(Who::Admin, ENABLE, json!({ "id": "dns" }))
        .await;
    assert_eq!(data(&dns)["state"], json!("enabled"));
    let listed = harness
        .call(Who::Admin, "keel.product.list", json!({}))
        .await;
    let states: Vec<(Value, Value)> = data(&listed)["data"]
        .as_array()
        .expect("product rows")
        .iter()
        .map(|row| (row["id"].clone(), row["state"].clone()))
        .collect();
    assert_eq!(
        states,
        [
            (json!("dns"), json!("enabled")),
            (json!("panel"), json!("disabled"))
        ]
    );
    assert!(!harness.modules().iter().any(|module| module == "hosting"));
    harness.finish().await;
}
