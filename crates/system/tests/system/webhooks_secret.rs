use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::SecretCipher;
use keel_system::webhooks::{
    MODULE, PRIVATE_TARGETS_KEY, SUBSCRIPTION_GET, SUBSCRIPTION_LIST, SUBSCRIPTION_SET,
};
use keel_testkit::{Harness, Reply, Who, tokio};

const SECRET: &str = "whsec_chosen_by_the_caller";

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

#[tokio::test]
async fn a_subscription_secret_is_answered_once_sealed_at_rest_and_masked_in_audit() {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .module(&keel_system::audit::MODULE)
        .module(&keel_system::settings::MODULE)
        .boot()
        .await;
    let allowed = harness
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": PRIVATE_TARGETS_KEY, "value": true }),
        )
        .await;
    data(&allowed);
    let created = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "sealed", "url": "http://127.0.0.1:9/in", "secret": SECRET, "events": ["note.*"] }),
        )
        .await;
    let created = data(&created).clone();
    assert_eq!(
        created["secret"],
        json!(SECRET),
        "answered to the call that set it"
    );
    let id = created["id"].as_str().expect("an id");
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT secret FROM webhook_subscription WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], SECRET);
    assert!(!stored[0].contains(SECRET), "{}", stored[0]);
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel provides the cipher");
    assert_eq!(
        cipher.open(&stored[0]).expect("the data key opens it"),
        SECRET.as_bytes()
    );
    let got = harness
        .call(Who::A1, SUBSCRIPTION_GET, json!({ "id": id }))
        .await;
    assert!(data(&got).get("secret").is_none(), "{:?}", got.data());
    let listed = harness.call(Who::A1, SUBSCRIPTION_LIST, json!({})).await;
    assert!(
        !serde_json::to_string(data(&listed))
            .expect("json")
            .contains(SECRET)
    );
    harness
        .registry()
        .services()
        .get::<keel_system::audit::api::Flush>()
        .expect("the audit module provides Flush")
        .flush()
        .await
        .expect("the worker answers the flush");
    let audited = harness
        .database()
        .rows(&format!(
            "SELECT json_build_object('params', params_redacted, 'after', after)::text FROM audit_call WHERE function = '{SUBSCRIPTION_SET}' AND status = 'committed' ORDER BY at"
        ))
        .await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert!(!audited[0].contains(SECRET), "{}", audited[0]);
    let entry: Value = serde_json::from_str(&audited[0]).expect("json");
    assert_eq!(entry["params"]["secret"], json!("***"));
    assert_eq!(entry["params"]["name"], json!("sealed"));
    assert_eq!(entry["after"]["secret"], json!("***"));
    assert_eq!(entry["after"]["name"], json!("sealed"));
    harness.finish().await;
}
