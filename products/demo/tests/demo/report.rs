use keel::prelude::*;
use keel_testkit::{Who, tokio};

const REPORT: &str = "note.report.tags";
const SERIES: &str = "note.report.series";

#[tokio::test]
async fn the_fixture_report_counts_the_tags_of_the_period_and_its_series_answer_labels_and_days() {
    let harness = crate::demo()
        .clock("2026-09-13T20:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    for (who, title, tags) in [
        (Who::A1, "a", vec!["work", "urgent"]),
        (Who::A1, "b", vec!["work"]),
        (Who::A1, "c", vec!["home", "work"]),
        (Who::A2, "e", vec!["work", "travel"]),
    ] {
        let saved = harness
            .call(who, "note.edit", json!({ "title": title, "tags": tags }))
            .await;
        assert!(saved.data().is_some(), "{who} {title}: {:?}", saved.error());
    }
    let report = harness.call(Who::A1, REPORT, json!({})).await;
    let report = report.data().expect("a report");
    assert_eq!(
        report["bands"]["tags"],
        json!([
            { "tag": "home", "notes": 1 },
            { "tag": "urgent", "notes": 1 },
            { "tag": "work", "notes": 3 }
        ])
    );
    assert_eq!(
        report["charts"],
        json!({ "created": {}, "tags": {}, "share": {} })
    );

    let later = harness
        .call(Who::A1, REPORT, json!({ "from": "2026-09-14" }))
        .await;
    let later = later.data().expect("a report");
    assert_eq!(later["bands"]["tags"], json!([]));
    assert_eq!(later["charts"]["created"], json!({ "from": "2026-09-14" }));

    let by_tag = harness
        .call(Who::A1, SERIES, json!({ "by": "tag", "to": "2026-09-13" }))
        .await;
    assert_eq!(
        by_tag.data().expect("a series")["points"],
        json!([
            { "label": "home", "value": 1 },
            { "label": "urgent", "value": 1 },
            { "label": "work", "value": 3 }
        ])
    );
    let by_day = harness
        .call(Who::Reseller, SERIES, json!({ "by": "day" }))
        .await;
    assert_eq!(
        by_day.data().expect("a series")["points"],
        json!([{ "at": "2026-09-13T00:00:00Z", "value": 4 }])
    );
    harness.finish().await;
}
