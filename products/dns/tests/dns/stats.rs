use keel::prelude::*;
use keel_system::stats::{COLLECT_JOB, SERIES};
use keel_testkit::{Harness, Who, tokio};

use crate::support::{Site, composed, create, data, run_job};

const ZONES: &str = "dns.zones";

async fn zones_of(harness: &Harness, who: Who) -> Vec<f64> {
    let reply = harness
        .call(
            who,
            SERIES,
            json!({
                "key": ZONES,
                "from": "2026-09-25T00:00:00Z",
                "to": "2026-09-25T01:00:00Z",
                "resolution": "raw",
            }),
        )
        .await;
    data(&reply)["points"]
        .as_array()
        .expect("the points")
        .iter()
        .map(|point| point["value"].as_f64().expect("a value"))
        .collect()
}

#[tokio::test]
async fn the_zone_count_is_sampled_per_account_every_hour() {
    let site = Site::new();
    let harness = composed(&site)
        .clock("2026-09-25T00:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    create(&harness, Who::A1, "one.example").await;
    create(&harness, Who::A1, "two.example").await;
    create(&harness, Who::A2, "three.example").await;
    run_job(&harness, COLLECT_JOB).await;
    assert_eq!(zones_of(&harness, Who::A1).await, [2.0]);
    assert_eq!(zones_of(&harness, Who::A2).await, [1.0]);
    assert!(zones_of(&harness, Who::Unrelated).await.is_empty());
    let keys = harness.call(Who::A1, "stats.keys", json!({})).await;
    assert_eq!(
        data(&keys)["keys"],
        json!([{ "key": ZONES, "unit": "count", "intervalSeconds": 3600, "scope": "account" }])
    );
    harness.finish().await;
}
