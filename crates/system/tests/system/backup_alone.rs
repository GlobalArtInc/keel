use keel_sdk::prelude::*;
use keel_sdk::types::Envelope;
use keel_system::backup::{FUNCTIONS, LIST, MENU_PATH, MODULE};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn backup_alone_boots_on_the_tasks_module_without_a_table_and_lists_an_empty_directory() {
    let harness = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&MODULE)
        .boot()
        .await;
    assert_eq!(harness.modules(), ["keel", "task", "backup", "testkit"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'backup\\_%' OR table_name LIKE 'keel\\_migration\\_backup%' ORDER BY table_name")
        .await;
    assert!(tables.is_empty(), "{tables:?}");
    let registered: Vec<&str> = harness
        .registry()
        .functions()
        .map(|function| function.name())
        .filter(|name| name.starts_with("backup."))
        .collect();
    assert_eq!(registered.len(), FUNCTIONS.len());
    for name in FUNCTIONS {
        assert!(registered.contains(&name), "{name} is registered");
    }
    assert!(
        harness
            .registry()
            .menu()
            .iter()
            .any(|node| node.path() == MENU_PATH && node.func() == LIST)
    );
    let socket = harness.socket().await;
    let listed = socket.call(LIST, json!({})).await;
    let Envelope::Ok { data, .. } = listed else {
        panic!("the list answers on the socket: {listed:?}");
    };
    assert_eq!(data["total"], json!(0));
    assert!(!harness.data_dir().join("backups").exists());
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
