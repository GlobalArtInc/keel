use keel::prelude::*;
use keel_demo::MANIFEST;
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const JSON: (&str, &str) = ("content-type", "application/json");

async fn list(http: &TestHttp, who: Who, key: &str, op: &str, value: &str) -> Vec<String> {
    let answer: Answer = http
        .request(
            "POST",
            "/api/v1/call/note.list",
            &[("authorization", &format!("Bearer {}", who.login())), JSON],
            Some(&json!({ "filters": [{ "key": key, "op": op, "value": value }] }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 200, "{who} {op} {value}: {}", answer.body);
    answer.json()["data"]["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().expect("a title").to_owned())
        .collect()
}

#[tokio::test]
async fn a_bare_date_filter_takes_the_day_boundary_in_the_callers_zone_and_utc_without_one() {
    let harness = Harness::new()
        .product(&MANIFEST)
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::settings::MODULE)
        .clock("2026-09-13T20:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    let a1 = harness.fixtures().accounts.a1;
    let a2 = harness.fixtures().accounts.a2;
    for account in [a1, a2] {
        let saved = harness
            .call(
                Who::Owner,
                "note.edit",
                json!({ "title": "late", "body": "", "account": account }),
            )
            .await;
        assert_eq!(saved.status(), 200, "{:?}", saved.error());
    }
    let zoned = harness
        .call(
            Who::A1,
            "settings.pref.set",
            json!({ "key": "tz", "value": "Asia/Tokyo" }),
        )
        .await;
    assert_eq!(zoned.status(), 200, "{:?}", zoned.error());

    let http = harness.http().await;
    assert_eq!(
        list(&http, Who::A1, "updated_at", "gte", "2026-09-14").await,
        ["late"]
    );
    assert!(
        list(&http, Who::A2, "updated_at", "gte", "2026-09-14")
            .await
            .is_empty()
    );
    assert!(
        list(&http, Who::A1, "updated_at", "lte", "2026-09-13")
            .await
            .is_empty()
    );
    assert_eq!(
        list(&http, Who::A2, "updated_at", "lte", "2026-09-13").await,
        ["late"]
    );
    assert_eq!(
        list(&http, Who::A1, "updated_at", "eq", "2026-09-14").await,
        ["late"]
    );
    assert_eq!(
        list(&http, Who::A2, "updated_at", "eq", "2026-09-13").await,
        ["late"]
    );
    for who in [Who::A1, Who::A2] {
        assert_eq!(
            list(&http, who, "updated_at", "gte", "2026-09-13T20:00:00Z").await,
            ["late"],
            "{who}"
        );
        assert!(
            list(&http, who, "updated_at", "gt", "2026-09-13T20:00:00Z")
                .await
                .is_empty(),
            "{who}"
        );
    }
    let vague = http
        .request(
            "POST",
            "/api/v1/call/note.list",
            &[("authorization", "Bearer a1"), JSON],
            Some(
                &json!({ "filters": [{ "key": "updated_at", "value": "yesterday" }] }).to_string(),
            ),
        )
        .await;
    assert_eq!(vague.status, 422, "{}", vague.body);
    assert_eq!(
        vague.json()["error"]["fields"][0]["path"],
        json!("filters[0].value")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
