use keel_sdk::prelude::*;
use keel_sdk::types::Envelope;
use keel_system::update::{
    CHECK, ERROR_CHANNEL, ERROR_CHANNEL_URL, MODULE, POLL_JOB, POLL_SCHEDULE, SECTION,
};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn update_alone_boots_with_its_table_its_job_and_names_an_unreachable_channel_on_the_socket()
{
    let harness = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .section(SECTION, json!({ "channel_url": "http://127.0.0.1:9/" }))
        .boot()
        .await;
    assert_eq!(
        harness.modules(),
        ["keel", "cron", "task", "testkit", "update"]
    );
    let applied = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_migration_update")
        .await;
    assert_eq!(applied, ["1"]);
    let tables = harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_name LIKE 'update\\_%' ORDER BY table_name")
        .await;
    assert_eq!(tables, ["update_run"]);
    let socket = harness.socket().await;
    let jobs = socket.call(keel_system::cron::JOB_LIST, json!({})).await;
    let Envelope::Ok { data, .. } = jobs else {
        panic!("the job list answers: {jobs:?}");
    };
    assert_eq!(data["total"], json!(1));
    assert_eq!(data["data"][0]["id"], json!(POLL_JOB));
    assert_eq!(data["data"][0]["module"], json!("update"));
    assert_eq!(data["data"][0]["schedule"], json!(POLL_SCHEDULE));
    let checked = socket.call(CHECK, json!({})).await;
    let Envelope::Err { error } = checked else {
        panic!("a closed port answers no channel: {checked:?}");
    };
    assert_eq!(error.key, ERROR_CHANNEL);
    assert_eq!(error.code.as_str(), "unavailable");
    assert_eq!(
        error.params["url"],
        json!("http://127.0.0.1:9/stable/manifest.json")
    );
    socket.stop().await.expect("the socket stops");

    let refused = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .section(SECTION, json!({ "channel_url": "file:///srv/channel" }))
        .try_boot()
        .await
        .expect_err("a channel that is not http does not boot")
        .to_string();
    assert!(refused.contains(ERROR_CHANNEL_URL), "{refused}");
    harness.finish().await;
}
