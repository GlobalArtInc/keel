use keel_sdk::prelude::*;
use keel_sdk::types::Envelope;
use keel_system::notify::{MESSAGE_LIST, MODULE, POLL_JOB, PROBLEM_LIST};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn notify_alone_boots_with_its_tables_its_poll_job_and_answers_its_lists_on_the_socket() {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .boot()
        .await;
    assert_eq!(harness.modules(), ["keel", "cron", "notify", "testkit"]);
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_notify")
        .await;
    assert_eq!(applied, ["5"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'notify\\_%' ORDER BY table_name")
        .await;
    assert_eq!(
        tables,
        [
            "notify_channel",
            "notify_cursor",
            "notify_delivery",
            "notify_message",
            "notify_problem"
        ]
    );
    let socket = harness.socket().await;
    for func in [MESSAGE_LIST, PROBLEM_LIST] {
        let listed = socket.call(func, json!({})).await;
        let page = match listed {
            Envelope::Ok { data, .. } => data,
            Envelope::Err { error } => panic!("{func}: {error:?}"),
        };
        assert_eq!(page["total"], json!(0), "{func}");
        assert_eq!(page["data"], json!([]), "{func}");
    }
    let jobs = socket.call(keel_system::cron::JOB_LIST, json!({})).await;
    let Envelope::Ok { data, .. } = jobs else {
        panic!("the job list answers: {jobs:?}");
    };
    assert_eq!(data["total"], json!(1));
    assert_eq!(data["data"][0]["id"], json!(POLL_JOB));
    assert_eq!(data["data"][0]["module"], json!("notify"));
    let output = socket.ctl_table(PROBLEM_LIST, &[], "en").await;
    assert!(output.ok, "{}", output.text);
    assert!(output.text.contains("Problem"), "{}", output.text);
    assert!(output.text.contains("Severity"), "{}", output.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
