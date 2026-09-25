use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use keel_release::{KeyRing, PublicKey, Signed};
use keel_sdk::chrono::{DateTime, Days, Utc};
use keel_sdk::prelude::*;
use keel_system::license::{ERROR_SIGNATURE, GET, INSTALL, MODULE};
use keel_testkit::license::verifying_key;
use keel_testkit::{ErrorCode, Harness, Who, tokio};

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

fn keel_dev(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_keel-dev"))
        .args(args)
        .output()
        .expect("keel-dev runs");
    assert!(
        output.status.success(),
        "keel-dev {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("keel-dev prints text")
}

fn path(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

fn issue(keys: &Path, instance: Id, out: &Path) {
    let instance = instance.to_string();
    let printed = keel_dev(&[
        "license",
        "issue",
        "--key",
        path(&keys.join("license.key")),
        "--instance",
        &instance,
        "--product",
        "orbit:plus",
        "--days",
        "365",
        "--grace",
        "30",
        "--out",
        path(out),
    ]);
    assert!(printed.contains(&instance), "{printed}");
    assert!(printed.contains("orbit:plus"), "{printed}");
}

async fn licensed(keys: &Path) -> Harness {
    let key = verifying_key(&keys.join("license.pub")).expect("the public half reads back");
    Harness::new()
        .product(&ORBIT)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .license_key(key)
        .boot()
        .await
}

#[tokio::test]
async fn an_issued_lease_verifies_with_the_matching_key_and_installs() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let keys = dir.path().join("keys");
    let stranger = dir.path().join("stranger");
    let printed = keel_dev(&["license", "keygen", "--out", path(&keys)]);
    let public = PublicKey::read(&keys.join("license.pub")).expect("a public half");
    assert!(printed.contains(&public.to_string()), "{printed}");
    keel_dev(&["license", "keygen", "--out", path(&stranger)]);

    let harness = licensed(&keys).await;
    let lease = dir.path().join("lease.json");
    issue(&keys, harness.instance(), &lease);
    let signed = fs::read_to_string(&lease).expect("the lease is written");

    let envelope = Signed::parse(signed.as_bytes()).expect("a signed envelope");
    let payload: Value = serde_json::from_slice(
        envelope
            .open(&KeyRing::new(public, Vec::new()))
            .expect("the matching public half verifies the lease"),
    )
    .expect("a JSON lease");
    let foreign = PublicKey::read(&stranger.join("license.pub")).expect("another public half");
    assert!(envelope.open(&KeyRing::new(foreign, Vec::new())).is_err());
    assert_eq!(payload["instance_id"], json!(harness.instance()));
    assert_eq!(payload["grace_days"], json!(30));
    let issued: DateTime<Utc> =
        serde_json::from_value(payload["issued_at"].clone()).expect("issued_at is an instant");
    let expires: DateTime<Utc> =
        serde_json::from_value(payload["expires_at"].clone()).expect("expires_at is an instant");
    assert_eq!(issued.checked_add_days(Days::new(365)), Some(expires));

    let installed = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    let answer = installed
        .data()
        .unwrap_or_else(|| panic!("the lease installs: {:?}", installed.error()));
    assert_eq!(answer["state"], json!("valid"));
    assert_eq!(answer["instanceId"], json!(harness.instance()));
    assert_eq!(
        answer["products"],
        json!([{ "id": "orbit", "edition": "plus" }])
    );
    assert_eq!(answer["expiresAt"], payload["expires_at"]);
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let read = harness.call(Who::Admin, GET, json!({})).await;
        if read.data().map(|data| &data["state"]) == Some(&json!("valid")) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "license.get did not answer valid within {PATIENCE:?}: {read:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let other = licensed(&stranger).await;
    let refused = other
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    let error = refused.error().expect("a lease of another key is refused");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(
        error.fields.first().and_then(|field| field.key.as_deref()),
        Some(ERROR_SIGNATURE),
        "{error:?}"
    );
}
