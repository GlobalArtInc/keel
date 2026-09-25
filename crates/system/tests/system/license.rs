use std::time::Duration;

use keel_sdk::chrono::{DateTime, Days, Utc};
use keel_sdk::prelude::*;
use keel_sdk::serde_json::Map;
use keel_sdk::system::Features;
use keel_sdk::types::LicenseState;
use keel_system::license::{
    ERROR_INSTANCE, ERROR_LEASE, ERROR_SIGNATURE, GET, INSTALL, KV_KEY, MODULE,
};
use keel_testkit::license::Issuer;
use keel_testkit::{ErrorCode, Harness, Reply, Who, tokio};

const NOW: &str = "2026-09-25T00:00:00Z";
const PATIENCE: Duration = Duration::from_secs(10);

static ORBIT: ProductManifest = ProductManifest {
    id: "orbit",
    name: "product.orbit.brand",
    version: "0.1.0",
    api: keel_sdk::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.orbit.logo",
        hue: 200,
        chroma: 0.1,
    },
    modules: &[],
    menu: &[],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[
        Edition {
            id: "basic",
            features: &["orbit.charts"],
        },
        Edition {
            id: "plus",
            features: &["orbit.charts", "orbit.reports"],
        },
    ],
    i18n: &[("en", r#"{ "product": { "orbit": { "brand": "Orbit" } } }"#)],
    assets: &[],
};

static COMET: ProductManifest = ProductManifest {
    id: "comet",
    name: "product.comet.brand",
    version: "0.1.0",
    api: keel_sdk::types::DESCRIPTOR_VERSION,
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
        id: "one",
        features: &["comet.maps"],
    }],
    i18n: &[("en", r#"{ "product": { "comet": { "brand": "Comet" } } }"#)],
    assets: &[],
};

static NOVA: ProductManifest = ProductManifest {
    id: "nova",
    name: "product.nova.brand",
    version: "0.1.0",
    api: keel_sdk::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.nova.logo",
        hue: 300,
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
        id: "solo",
        features: &["nova.stars"],
    }],
    i18n: &[("en", r#"{ "product": { "nova": { "brand": "Nova" } } }"#)],
    assets: &[],
};

fn now() -> DateTime<Utc> {
    NOW.parse().expect("an instant")
}

fn throwaway() -> Issuer {
    Issuer::generate().expect("a throwaway license key")
}

async fn licensed(issuer: &Issuer) -> Harness {
    Harness::new()
        .product(&ORBIT)
        .product(&COMET)
        .product(&NOVA)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .license_key(issuer.key())
        .clock(now())
        .boot()
        .await
}

fn lease(instance: Id, products: &[(&str, &str)]) -> Value {
    json!({
        "instance_id": instance,
        "issued_at": now(),
        "expires_at": now() + Days::new(365),
        "grace_days": 30,
        "products": products
            .iter()
            .map(|(id, edition)| json!({ "id": id, "edition": edition }))
            .collect::<Vec<_>>(),
    })
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused<'a>(reply: &'a Reply, key: &str) -> &'a Map<String, Value> {
    let error = reply.error().expect("the lease is refused");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(reply.status(), 422);
    let field = error.fields.first().expect("a field error");
    assert_eq!(field.path, "lease", "{error:?}");
    assert_eq!(field.key.as_deref(), Some(key), "{error:?}");
    &field.params
}

fn features(harness: &Harness) -> Vec<String> {
    let mut features = harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set")
        .enabled();
    features.sort();
    features
}

async fn settled(harness: &Harness, wanted: LicenseState) {
    let features = harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while features.license().map(|license| license.state) != Some(wanted) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the license did not become {wanted:?} within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn state(harness: &Harness) -> Value {
    data(&harness.call(Who::Admin, GET, json!({})).await)["state"].clone()
}

fn tampered(signed: &str) -> String {
    let mut envelope: Value = keel_sdk::serde_json::from_str(signed).expect("an envelope");
    let payload = envelope["payload"].as_str().expect("a payload").to_owned();
    let mut chars: Vec<char> = payload.chars().collect();
    chars[10] = if chars[10] == 'A' { 'B' } else { 'A' };
    envelope["payload"] = Value::String(chars.into_iter().collect());
    envelope.to_string()
}

#[tokio::test]
async fn a_lease_signed_with_the_license_key_installs_and_a_changed_byte_does_not() {
    let issuer = throwaway();
    let harness = licensed(&issuer).await;
    let signed = issuer
        .sign(&lease(harness.instance(), &[("orbit", "plus")]))
        .expect("the lease is signed");
    assert_eq!(state(&harness).await, json!("none"));

    refused(
        &harness
            .call(Who::Admin, INSTALL, json!({ "lease": tampered(&signed) }))
            .await,
        ERROR_SIGNATURE,
    );
    let foreign = throwaway()
        .sign(&lease(harness.instance(), &[("orbit", "plus")]))
        .expect("a lease of another key");
    refused(
        &harness
            .call(Who::Admin, INSTALL, json!({ "lease": foreign }))
            .await,
        ERROR_SIGNATURE,
    );
    let shapeless = issuer
        .sign(&json!({ "instance_id": harness.instance() }))
        .expect("a signed document that is not a lease");
    refused(
        &harness
            .call(Who::Admin, INSTALL, json!({ "lease": shapeless }))
            .await,
        ERROR_LEASE,
    );
    let stored = format!("SELECT count(*)::text FROM keel_kv WHERE key = '{KV_KEY}'");
    assert_eq!(harness.database().rows(&stored).await, ["0"]);
    assert_eq!(state(&harness).await, json!("none"));

    let user = harness
        .call(Who::A1, INSTALL, json!({ "lease": signed }))
        .await;
    assert_eq!(user.status(), 403, "{:?}", user.error());

    let installed = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    let answer = data(&installed);
    assert_eq!(answer["state"], json!("valid"));
    settled(&harness, LicenseState::Valid).await;
    assert_eq!(answer["instanceId"], json!(harness.instance()));
    assert_eq!(
        answer["products"],
        json!([{ "id": "orbit", "edition": "plus" }])
    );
    assert_eq!(answer["expiresAt"], json!(now() + Days::new(365)));
    assert_eq!(answer["graceEndsAt"], json!(now() + Days::new(395)));
    let text = format!("SELECT value #>> '{{}}' FROM keel_kv WHERE key = '{KV_KEY}'");
    assert_eq!(harness.database().rows(&text).await, [signed]);
    assert_eq!(
        data(&harness.call(Who::Admin, GET, json!({})).await),
        answer
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_lease_for_another_instance_id_is_refused() {
    let issuer = throwaway();
    let harness = licensed(&issuer).await;
    let other = Id::generate();
    let signed = issuer
        .sign(&lease(other, &[("orbit", "plus")]))
        .expect("the lease is signed");

    let reply = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    assert_eq!(
        refused(&reply, ERROR_INSTANCE)["instance"],
        json!(other.to_string())
    );
    assert_eq!(state(&harness).await, json!("none"));
    assert_eq!(
        features(&harness),
        ["comet.maps", "nova.stars", "orbit.charts"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_feature_set_is_the_union_of_the_listed_editions() {
    let issuer = throwaway();
    let harness = licensed(&issuer).await;
    let http = harness.http().await;
    let before = http.get(Who::Admin, "/api/v1/boot").await;
    assert_eq!(before.status, 200, "{}", before.body);
    let boot = before.json();
    assert_eq!(
        boot["license"],
        json!({ "state": "none", "expiresAt": null, "graceEndsAt": null })
    );
    assert_eq!(
        features(&harness),
        ["comet.maps", "nova.stars", "orbit.charts"]
    );

    let signed = issuer
        .sign(&lease(
            harness.instance(),
            &[("orbit", "plus"), ("comet", "one")],
        ))
        .expect("the lease is signed");
    let installed = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    assert_eq!(data(&installed)["state"], json!("valid"));
    settled(&harness, LicenseState::Valid).await;

    assert_eq!(
        features(&harness),
        ["comet.maps", "orbit.charts", "orbit.reports"]
    );
    let after = http.get(Who::Admin, "/api/v1/boot").await;
    assert_eq!(after.status, 200, "{}", after.body);
    let boot = after.json();
    let mut listed: Vec<&str> = boot["features"]
        .as_array()
        .expect("features")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    listed.sort_unstable();
    assert_eq!(listed, ["comet.maps", "orbit.charts", "orbit.reports"]);
    let editions: Vec<(&str, &Value)> = boot["products"]
        .as_array()
        .expect("products")
        .iter()
        .map(|product| (product["id"].as_str().expect("an id"), &product["edition"]))
        .collect();
    assert_eq!(
        editions,
        [
            ("orbit", &json!("plus")),
            ("comet", &json!("one")),
            ("nova", &Value::Null),
        ]
    );
    assert_eq!(
        boot["license"],
        json!({
            "state": "valid",
            "expiresAt": now() + Days::new(365),
            "graceEndsAt": now() + Days::new(395),
        })
    );
    assert_ne!(boot["meta"], before.json()["meta"]);
    harness.finish().await;
}

#[tokio::test]
async fn license_none_keeps_every_feature_of_every_enabled_product() {
    let harness = Harness::new()
        .product(&ORBIT)
        .product(&COMET)
        .product(&NOVA)
        .clock(now())
        .boot()
        .await;
    assert!(!harness.modules().iter().any(|module| module == "license"));
    assert_eq!(
        features(&harness),
        ["comet.maps", "nova.stars", "orbit.charts", "orbit.reports"]
    );
    let http = harness.http().await;
    let boot = http.get(Who::Admin, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    let boot = boot.json();
    assert_eq!(boot["license"], Value::Null);
    let editions: Vec<&Value> = boot["products"]
        .as_array()
        .expect("products")
        .iter()
        .map(|product| &product["edition"])
        .collect();
    assert_eq!(editions, [&json!("basic"), &json!("one"), &json!("solo")]);
    let unknown = harness
        .call(Who::Admin, INSTALL, json!({ "lease": "{}" }))
        .await;
    assert_eq!(unknown.status(), 404, "{:?}", unknown.error());
    harness.finish().await;
}
