use keel::prelude::*;
use keel_system::stats::{COLLECT_JOB, SERIES};
use keel_testkit::{Who, tokio};

use crate::support::{Site, composed, create, data, run_job};

const WIDGET: &str = "dns.widget.zones";

#[tokio::test]
async fn the_zones_widget_counts_the_zones_in_scope_and_names_the_series_of_its_sparkline() {
    let site = Site::new();
    let harness = composed(&site)
        .clock("2026-09-25T00:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    let accounts = harness.fixtures().accounts.clone();
    create(&harness, Who::A1, "one.example").await;
    create(&harness, Who::A1, "two.example").await;
    create(&harness, Who::A2, "three.example").await;
    create(&harness, Who::Unrelated, "four.example").await;

    let own = harness.call(Who::A1, WIDGET, json!({})).await;
    assert_eq!(
        data(&own),
        &json!({
            "value": 2.0,
            "unit": "count",
            "series": "dns.zones",
            "subject": accounts.a1.to_string()
        })
    );
    let everyone = harness.call(Who::Admin, WIDGET, json!({})).await;
    assert_eq!(
        data(&everyone),
        &json!({ "value": 4.0, "unit": "count", "series": "dns.zones.total" })
    );
    let customers = harness.call(Who::Reseller, WIDGET, json!({})).await;
    assert_eq!(data(&customers), &json!({ "value": 3.0, "unit": "count" }));

    run_job(&harness, COLLECT_JOB).await;
    let series = harness
        .call(
            Who::Admin,
            SERIES,
            json!({
                "key": "dns.zones.total",
                "from": "2026-09-25T00:00:00Z",
                "to": "2026-09-25T01:00:00Z",
                "resolution": "raw",
            }),
        )
        .await;
    assert_eq!(data(&series)["points"][0]["value"], json!(4.0));

    let http = harness.http().await;
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    assert_eq!(
        boot.json()["dashboard"],
        json!([{ "kind": "stat", "func": WIDGET }])
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
