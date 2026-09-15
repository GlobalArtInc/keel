use keel_sdk::prelude::*;
use keel_system::cron::api::Scheduler;
use keel_system::cron::{JOB_LIST, LOCK_KEY, MODULE, RUN_LIST};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn cron_alone_boots_with_its_table_holds_its_lock_and_answers_cron_job_list_on_the_socket() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "cron", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_cron")
        .await;
    assert_eq!(applied, ["1"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'cron\\_%' ORDER BY table_name")
        .await;
    assert_eq!(tables, ["cron_run"]);

    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let mut held = Vec::new();
    for _ in 0..100 {
        held = harness
            .database()
            .rows(&format!(
                "SELECT count(*)::text FROM pg_locks WHERE locktype = 'advisory' AND granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) AND ((classid::bigint << 32) | objid::bigint) = {LOCK_KEY}"
            ))
            .await;
        if held == ["1"] && scheduler.is_leader() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(held, ["1"], "the leader holds the keelcron lock");
    assert!(scheduler.is_leader());
    assert!(scheduler.jobs().is_empty());
    assert!(scheduler.running().is_empty());

    let socket = harness.socket().await;
    for func in [JOB_LIST, RUN_LIST] {
        let listed = socket.call(func, json!({})).await;
        let page = match listed {
            keel_sdk::types::Envelope::Ok { data, .. } => data,
            keel_sdk::types::Envelope::Err { error } => panic!("{func}: {error:?}"),
        };
        assert_eq!(page["total"], json!(0), "{func}");
        assert_eq!(page["data"], json!([]), "{func}");
    }
    let output = socket.ctl_table(JOB_LIST, &[], "en").await;
    assert!(output.ok, "{}", output.text);
    assert!(output.text.contains("Job"), "{}", output.text);
    assert!(output.text.contains("Schedule"), "{}", output.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
