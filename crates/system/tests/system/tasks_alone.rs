use keel_sdk::prelude::*;
use keel_system::tasks::{LOCK_KEY, MODULE};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn tasks_alone_boots_with_its_tables_holds_its_lock_and_answers_task_list_on_the_socket() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "task", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_task")
        .await;
    assert_eq!(applied, ["2"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'task\\_%' ORDER BY table_name")
        .await;
    assert_eq!(tables, ["task_job", "task_step"]);

    let mut held = Vec::new();
    for _ in 0..100 {
        held = harness
            .database()
            .rows(&format!(
                "SELECT count(*)::text FROM pg_locks WHERE locktype = 'advisory' AND granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) AND ((classid::bigint << 32) | objid::bigint) = {LOCK_KEY}"
            ))
            .await;
        if held == ["1"] {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(held, ["1"], "the worker holds the keeltask lock");

    let socket = harness.socket().await;
    let listed = socket.call("task.list", json!({})).await;
    let page = match listed {
        keel_sdk::types::Envelope::Ok { data, .. } => data,
        keel_sdk::types::Envelope::Err { error } => panic!("{error:?}"),
    };
    assert_eq!(page["total"], json!(0));
    assert_eq!(page["data"], json!([]));
    let output = socket.ctl_table("task.list", &[], "en").await;
    assert!(output.ok, "{}", output.text);
    assert!(output.text.contains("Function"), "{}", output.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
