use keel_sdk::prelude::*;
use keel_sdk::sea_orm::ConnectionTrait;
use keel_sdk::serde_json;
use keel_sdk::types::{ErrorCode, InstanceInfo};
use keel_system::peers::{ERROR_STORED, FEATURE, GET, LIST, MODULE, STATUS_ACTIVE};
use keel_testkit::{Harness, Who, tokio};

const OLDER: &str = "01J9A00000000000000000000A";
const NEWER: &str = "01J9B00000000000000000000B";

async fn link(harness: &Harness, id: &str, url: &str, products: &str, created_at: &str) {
    harness
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO peer_product (id, instance_id, products_json, keel_version, protocol, url, \
             public_key, inbound_token_hash, inbound_scopes_json, outbound_token_enc, \
             webhook_secret_enc, status, last_seen_at, created_at) VALUES ('{id}', 'instance-{id}', \
             '{products}', '0.4.0', 1, '{url}', 'ed25519:MCowBQYDK2VwAyEA{id}', 'hash-{id}', \
             '{{\"functions\": [\"svc.*\"], \"level\": \"admin\", \"onBehalf\": true}}', \
             'sealed-token-{id}', 'sealed-secret-{id}', '{STATUS_ACTIVE}', NULL, '{created_at}')"
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_handshake_names_the_peers_feature() {
    let harness = Harness::new().module(&MODULE).boot().await;
    let http = harness.http().await;

    let answer = http.get(Who::Guest, "/api/v1/instance").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let info: InstanceInfo = serde_json::from_str(&answer.body).unwrap();
    assert!(
        info.features.iter().any(|feature| feature == FEATURE),
        "{info:?}"
    );
    assert_eq!(info.public_key, harness.instance_key().public_key());

    drop(http);
    harness.finish().await;
}

#[tokio::test]
async fn peer_list_and_get_answer_the_stored_links_without_their_secrets() {
    let harness = Harness::new().module(&MODULE).boot().await;
    link(
        &harness,
        OLDER,
        "https://dns.example:8443",
        r#"["dns"]"#,
        "2026-09-20T10:00:00Z",
    )
    .await;
    link(
        &harness,
        NEWER,
        "https://bill.example",
        r#"["bill", "dns"]"#,
        "2026-09-21T10:00:00Z",
    )
    .await;

    let listed = harness.call(Who::Admin, LIST, json!({})).await;
    assert_eq!(listed.status(), 200, "{listed:?}");
    let data = listed.data().unwrap();
    assert_eq!(data["total"], json!(2));
    let rows = data["data"].as_array().unwrap();
    let ids: Vec<&Value> = rows.iter().map(|row| &row["id"]).collect();
    assert_eq!(ids, [&json!(NEWER), &json!(OLDER)]);
    assert_eq!(
        rows[0],
        json!({
            "id": NEWER,
            "instanceId": format!("instance-{NEWER}"),
            "products": ["bill", "dns"],
            "keel": "0.4.0",
            "protocol": 1,
            "url": "https://bill.example",
            "publicKey": format!("ed25519:MCowBQYDK2VwAyEA{NEWER}"),
            "scopes": { "functions": ["svc.*"], "level": "admin", "onBehalf": true },
            "status": STATUS_ACTIVE,
            "lastSeenAt": null,
            "createdAt": "2026-09-21T10:00:00Z"
        })
    );
    let text = serde_json::to_string(data).unwrap();
    for secret in ["hash-", "sealed-token-", "sealed-secret-"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }

    let got = harness.call(Who::Admin, GET, json!({ "id": OLDER })).await;
    assert_eq!(got.status(), 200, "{got:?}");
    assert_eq!(got.data().unwrap(), &rows[1]);

    let missing = harness
        .call(
            Who::Admin,
            GET,
            json!({ "id": "01J9C00000000000000000000C" }),
        )
        .await;
    assert_eq!(missing.error().unwrap().code, ErrorCode::NotFound);

    for who in [Who::Reseller, Who::A1] {
        let refused = harness.call(who, LIST, json!({})).await;
        assert_eq!(
            refused.error().unwrap().code,
            ErrorCode::Forbidden,
            "{refused:?}"
        );
    }

    harness.finish().await;
}

#[tokio::test]
async fn an_unreadable_link_answers_the_stored_error() {
    let harness = Harness::new().module(&MODULE).boot().await;
    link(
        &harness,
        OLDER,
        "https://dns.example:8443",
        r#"{"dns": true}"#,
        "2026-09-20T10:00:00Z",
    )
    .await;

    let got = harness.call(Who::Admin, GET, json!({ "id": OLDER })).await;
    let error = got.error().unwrap();
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.key, ERROR_STORED);

    harness.finish().await;
}
