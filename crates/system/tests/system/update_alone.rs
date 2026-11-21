use keel_sdk::prelude::*;
use keel_sdk::types::Envelope;
use keel_system::update::{
    CHECK, ERROR_CHANNEL, ERROR_CHANNEL_INSECURE, ERROR_CHANNEL_URL, MODULE, POLL_JOB,
    POLL_SCHEDULE, SECTION,
};
use keel_testkit::{Harness, tokio};

#[tokio::test]
async fn update_alone_boots_with_its_table_its_job_and_names_an_unreachable_channel_on_the_socket()
{
    let harness = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .section(
            SECTION,
            json!({ "channel_url": "http://127.0.0.1:9/", "insecure_rehearsal_channel": true }),
        )
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
    assert_eq!(data["total"], json!(2));
    assert_eq!(
        data["data"][0]["id"],
        json!(keel_system::cron::IDEMPOTENCY_JOB)
    );
    assert_eq!(data["data"][1]["id"], json!(POLL_JOB));
    assert_eq!(data["data"][1]["module"], json!("update"));
    assert_eq!(data["data"][1]["schedule"], json!(POLL_SCHEDULE));
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

    harness.finish().await;

    for (section, key) in [
        (
            json!({ "channel_url": "file:///srv/channel" }),
            ERROR_CHANNEL_URL,
        ),
        (
            json!({ "channel_url": "http://127.0.0.1:9/" }),
            ERROR_CHANNEL_INSECURE,
        ),
    ] {
        let kept = Harness::new()
            .module(&keel_system::tasks::MODULE)
            .module(&keel_system::cron::MODULE)
            .module(&MODULE)
            .section(SECTION, section)
            .boot()
            .await;
        let socket = kept.socket().await;
        let checked = socket.call(CHECK, json!({})).await;
        let Envelope::Err { error } = checked else {
            panic!("an unusable channel address answers an error: {checked:?}");
        };
        assert_eq!(error.key, key);
        let jobs = socket.call(keel_system::cron::JOB_LIST, json!({})).await;
        assert!(
            matches!(jobs, Envelope::Ok { .. }),
            "the kernel keeps running: {jobs:?}"
        );
        socket.stop().await.expect("the socket stops");
        kept.finish().await;
    }
}
