use keel_sdk::prelude::*;
use keel_sdk::types::Envelope;
use keel_system::webhooks::{DELIVERY_LIST, MODULE, SUBSCRIPTION_LIST};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn webhooks_alone_boots_with_its_tables_and_answers_its_lists_on_the_socket() {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .boot()
        .await;
    assert_eq!(
        harness.modules(),
        ["keel", "cron", "notify", "testkit", "webhook"]
    );
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_webhook")
        .await;
    assert_eq!(applied, ["4"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'webhook\\_%' ORDER BY table_name")
        .await;
    assert_eq!(
        tables,
        ["webhook_cursor", "webhook_delivery", "webhook_subscription"]
    );
    let socket = harness.socket().await;
    for func in [SUBSCRIPTION_LIST, DELIVERY_LIST] {
        let listed = socket.call(func, json!({})).await;
        let page = match listed {
            Envelope::Ok { data, .. } => data,
            Envelope::Err { error } => panic!("{func}: {error:?}"),
        };
        assert_eq!(page["total"], json!(0), "{func}");
        assert_eq!(page["data"], json!([]), "{func}");
    }
    let output = socket.ctl_table(SUBSCRIPTION_LIST, &[], "en").await;
    assert!(output.ok, "{}", output.text);
    assert!(output.text.contains("Name"), "{}", output.text);
    assert!(output.text.contains("URL"), "{}", output.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
