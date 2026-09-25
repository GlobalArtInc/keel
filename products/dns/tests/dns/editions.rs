use std::time::Duration;

use keel::prelude::*;
use keel::sdk::chrono::Days;
use keel::sdk::system::Features;
use keel::types::ErrorCode;
use keel_system::license::{INSTALL, MODULE};
use keel_testkit::license::Issuer;
use keel_testkit::{Harness, TestHttp, Who, tokio};

use crate::support::{Site, composed, create, data};

const DNSSEC_GET: &str = "dns.zone.dnssec.get";
const DNSSEC: &str = "dns.dnssec";

async fn leased(harness: &Harness, issuer: &Issuer, edition: &str) {
    let now = harness.clock().now();
    let signed = issuer
        .sign(&json!({
            "instance_id": harness.instance(),
            "issued_at": now,
            "expires_at": now + Days::new(365),
            "grace_days": 30,
            "products": [{ "id": "dns", "edition": edition }],
        }))
        .expect("the lease is signed");
    let installed = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    data(&installed);
    let features = harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set");
    let wanted = edition == "pro";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while features.allows(DNSSEC) != wanted {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the {edition} lease did not settle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn row_actions(http: &TestHttp) -> Vec<String> {
    let list = http.get(Who::A1, "/api/v1/functions/dns.zone.list").await;
    assert_eq!(list.status, 200, "{}", list.body);
    list.json()["ui"]["row_actions"]
        .as_array()
        .expect("row actions")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

async fn listed(http: &TestHttp) -> bool {
    let listing = http.get(Who::A1, "/api/v1/functions").await;
    assert_eq!(listing.status, 200, "{}", listing.body);
    listing
        .json()
        .as_array()
        .expect("a descriptor array")
        .iter()
        .any(|descriptor| descriptor["name"] == json!(DNSSEC_GET))
}

async fn boot_features(http: &TestHttp) -> Vec<String> {
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    boot.json()["features"]
        .as_array()
        .expect("features")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn a_lite_lease_hides_dns_zone_dnssec_get_from_the_menu_and_it_answers_license() {
    let site = Site::new();
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = composed(&site)
        .module(&MODULE)
        .license_key(issuer.key())
        .boot()
        .await;
    let http = harness.http().await;
    leased(&harness, &issuer, "lite").await;
    let zone = create(&harness, Who::A1, "lite.example").await;

    let refused = harness
        .call(Who::A1, DNSSEC_GET, json!({ "id": zone }))
        .await;
    let error = refused.error().expect("the lite lease refuses DNSSEC");
    assert_eq!(error.code, ErrorCode::License, "{error:?}");
    assert_eq!(refused.status(), 402);
    assert_eq!(error.key, "keel.error.license");
    assert_eq!(error.params["feature"], json!(DNSSEC));
    let descriptor = http
        .get(Who::A1, &format!("/api/v1/functions/{DNSSEC_GET}"))
        .await;
    assert_eq!(descriptor.status, 402, "{}", descriptor.body);
    assert!(!listed(&http).await);
    assert_eq!(
        row_actions(&http).await,
        ["edit", "records", "log", "delete"]
    );
    assert_eq!(boot_features(&http).await, ["dns.zones"]);

    leased(&harness, &issuer, "pro").await;
    let served = harness
        .call(Who::A1, DNSSEC_GET, json!({ "id": zone }))
        .await;
    assert_eq!(data(&served), &json!({ "state": "unsigned" }));
    let descriptor = http
        .get(Who::A1, &format!("/api/v1/functions/{DNSSEC_GET}"))
        .await;
    assert_eq!(descriptor.status, 200, "{}", descriptor.body);
    assert_eq!(descriptor.json()["access"]["feature"], json!(DNSSEC));
    assert!(listed(&http).await);
    assert_eq!(
        row_actions(&http).await,
        ["edit", "records", "dnssec", "log", "delete"]
    );
    assert_eq!(boot_features(&http).await, ["dns.zones", DNSSEC]);
    harness.finish().await;
}
