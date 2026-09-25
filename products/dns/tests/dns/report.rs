use keel::prelude::*;
use keel::types::ErrorCode;
use keel_testkit::{Harness, Who, tokio};

use crate::support::{Site, composed, create, data};

const REPORT: &str = "dns.report.zones";

fn rows(reply: &keel_testkit::Reply) -> Vec<Value> {
    let mut rows = data(reply)["bands"]["accounts"]
        .as_array()
        .expect("the accounts band")
        .clone();
    rows.sort_by_key(|row| row["accountId"].as_str().unwrap_or_default().to_owned());
    rows
}

fn row(account: Id, zones: u64, created: u64) -> Value {
    json!({ "accountId": account.to_string(), "zones": zones, "created": created })
}

async fn report(harness: &Harness, who: Who, params: Value) -> keel_testkit::Reply {
    harness.call(who, REPORT, params).await
}

#[tokio::test]
async fn the_zones_report_counts_the_zones_of_each_account_in_the_callers_scope_over_the_period() {
    let site = Site::new();
    let harness = composed(&site)
        .clock("2026-09-25T12:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    let accounts = harness.fixtures().accounts.clone();
    create(&harness, Who::A1, "one.example").await;
    create(&harness, Who::A1, "two.example").await;
    create(&harness, Who::A2, "three.example").await;
    create(&harness, Who::Unrelated, "four.example").await;

    let own = report(&harness, Who::A1, json!({})).await;
    assert_eq!(rows(&own), [row(accounts.a1, 2, 2)]);
    assert_eq!(
        data(&own)["charts"]["zones"],
        json!({ "from": "2026-08-27T00:00:00Z", "to": "2026-09-26T00:00:00Z" })
    );

    let earlier = report(&harness, Who::A1, json!({ "to": "2026-09-24" })).await;
    assert!(rows(&earlier).is_empty(), "{:?}", rows(&earlier));

    let everyone = report(&harness, Who::Admin, json!({ "from": "2026-09-25" })).await;
    let mut expected = vec![
        row(accounts.a1, 2, 2),
        row(accounts.a2, 1, 1),
        row(accounts.unrelated, 1, 1),
    ];
    expected.sort_by_key(|row| row["accountId"].as_str().unwrap_or_default().to_owned());
    assert_eq!(rows(&everyone), expected);

    let picked = report(&harness, Who::Admin, json!({ "account": accounts.a2 })).await;
    assert_eq!(rows(&picked), [row(accounts.a2, 1, 1)]);
    assert_eq!(
        data(&picked)["charts"]["zones"]["subject"],
        json!(accounts.a2.to_string())
    );

    let above = report(&harness, Who::A1, json!({ "account": accounts.a2 })).await;
    assert_eq!(
        above.error().map(|error| error.code),
        Some(ErrorCode::ForbiddenField)
    );

    let reversed = report(
        &harness,
        Who::A1,
        json!({ "from": "2026-09-20", "to": "2026-09-10" }),
    )
    .await;
    let error = reversed.error().expect("a refusal");
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("dns.report.error.period")
    );
    harness.finish().await;
}
