use std::time::Duration;

use keel::prelude::*;
use keel::sdk::futures::StreamExt;
use keel::types::ErrorCode;
use keel_testkit::{Who, tokio};

use crate::support::{Site, create, dns, rows, state_becomes};

const FOLLOW: Duration = Duration::from_secs(10);

#[tokio::test]
async fn the_zone_log_streams_the_apply_history_and_follows_the_next_apply() {
    let site = Site::new();
    let harness = dns(&site).await;
    let zone = create(&harness, Who::A1, "log.example").await;
    state_becomes(&harness, &zone, "active").await;
    let serial = rows(
        &harness,
        &format!("SELECT serial::text FROM dns_zone WHERE id = '{zone}'"),
    )
    .await;

    let mut opened = harness
        .call(Who::A1, "dns.zone.log", json!({ "id": zone }))
        .await;
    assert!(opened.error().is_none(), "{:?}", opened.error());
    let mut lines = opened
        .lines
        .take()
        .expect("the stream hands its lines over");
    let first = lines.next().await.expect("the backlog");
    assert_eq!(first.level, LogLevel::Info);
    assert_eq!(first.text, format!("serial {} applied", serial[0]));
    assert!(first.at.is_some());

    site.fail_checks(true);
    let edited = harness
        .call(
            Who::A1,
            "dns.zone.edit",
            json!({ "id": zone, "refresh": 7200, "retry": 3600, "expire": 604800, "minimum": 3600 }),
        )
        .await;
    assert!(edited.data().is_some(), "{:?}", edited.error());
    let next = tokio::time::timeout(FOLLOW, lines.next())
        .await
        .expect("the failed apply arrives")
        .expect("a line");
    assert_eq!(next.level, LogLevel::Error);
    state_becomes(&harness, &zone, "error").await;
    let error = rows(
        &harness,
        &format!("SELECT serial::text || ' failed: ' || error FROM dns_zone WHERE id = '{zone}'"),
    )
    .await;
    assert_eq!(next.text, format!("serial {}", error[0]));

    let refused = harness
        .call(Who::A2, "dns.zone.log", json!({ "id": zone }))
        .await;
    assert_eq!(
        refused.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    assert!(refused.lines.is_none());
}
