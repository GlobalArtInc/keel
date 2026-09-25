use std::future::Future;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::serde_json;
use keel_demo::note::{OVERDUE_PROBLEM, overdue};
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::notify::{POLL_JOB, PROBLEM_LIST};
use keel_testkit::{Harness, Reply, Who, tokio};

const PATIENCE: Duration = Duration::from_secs(15);

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn run_poll(harness: &Harness) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    wait_until("the leader takes the lock", || {
        let scheduler = scheduler.clone();
        async move { scheduler.is_leader() }
    })
    .await;
    let started = scheduler.run_now(POLL_JOB).await.expect("the poll starts");
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the poll ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok);
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || (params_json->>'count') || ' ' || coalesce(account_id, '-') FROM notify_problem WHERE cleared_at IS NULL ORDER BY subject")
        .await
}

async fn save(harness: &Harness, who: Who, params: Value) -> Id {
    let reply = harness.call(who, "note.edit", params).await;
    data(&reply)["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a ulid")
}

#[tokio::test]
async fn an_overdue_todo_raises_note_overdue_for_its_account_after_the_poll_and_a_done_one_clears_it()
 {
    let harness = crate::demo().boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let a2 = harness.fixtures().accounts.a2;
    let overdue_note = save(
        &harness,
        Who::A1,
        json!({ "title": "renew the certificate", "kind": "todo", "due": "2020-01-01" }),
    )
    .await;
    save(
        &harness,
        Who::A1,
        json!({ "title": "water the plants", "kind": "todo", "due": "2020-02-01" }),
    )
    .await;
    save(
        &harness,
        Who::A1,
        json!({ "title": "someday", "kind": "todo", "due": "2999-01-01" }),
    )
    .await;
    save(
        &harness,
        Who::A2,
        json!({ "title": "a memo", "kind": "memo" }),
    )
    .await;
    save(
        &harness,
        Who::A2,
        json!({ "title": "late too", "kind": "todo", "due": "2021-06-01" }),
    )
    .await;
    let db = keel::sdk::Db::from_connection(harness.db().clone());
    let reported = overdue(&db).await.expect("the source polls");
    let seen: Vec<(String, String, Value, Option<Id>)> = reported
        .iter()
        .map(|raise| {
            (
                raise.key.clone(),
                raise.subject.clone(),
                raise.params.clone(),
                raise.account,
            )
        })
        .collect();
    let mut expected = vec![
        (
            OVERDUE_PROBLEM.to_owned(),
            a1.to_string(),
            json!({ "count": 2 }),
            Some(a1),
        ),
        (
            OVERDUE_PROBLEM.to_owned(),
            a2.to_string(),
            json!({ "count": 1 }),
            Some(a2),
        ),
    ];
    expected.sort_by(|left, right| left.1.cmp(&right.1));
    assert_eq!(seen, expected);
    assert!(open_problems(&harness).await.is_empty());

    run_poll(&harness).await;
    let mut problems = vec![
        format!("{OVERDUE_PROBLEM} {a1} 2 {a1}"),
        format!("{OVERDUE_PROBLEM} {a2} 1 {a2}"),
    ];
    problems.sort();
    assert_eq!(open_problems(&harness).await, problems);
    let a1_list = harness.call(Who::A1, PROBLEM_LIST, json!({})).await;
    assert_eq!(data(&a1_list)["total"], json!(1));
    assert_eq!(data(&a1_list)["data"][0]["key"], json!(OVERDUE_PROBLEM));
    assert_eq!(data(&a1_list)["data"][0]["params"], json!({ "count": 2 }));
    assert_eq!(data(&a1_list)["data"][0]["subject"], json!(a1.to_string()));

    let http = harness.http().await;
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.status, 200, "{}", boot.body);
    let boot = boot.json();
    assert_eq!(boot["problems"], json!({ "open": 1, "critical": 0 }));
    let notes = boot["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["items"].as_array().into_iter().flatten())
        .find(|item| item["path"] == json!("demo/notes"))
        .cloned()
        .expect("the notes node");
    assert_eq!(notes["badge"], json!({ "problem": OVERDUE_PROBLEM }));
    let unread = http
        .post(
            Who::A1,
            "/api/v1/call/notify.message.list",
            &json!({ "filters": [{ "key": "unread", "op": "eq", "value": "true" }] }),
        )
        .await;
    assert_eq!(unread.status, 200, "{}", unread.body);
    let page = unread.json();
    assert_eq!(page["data"]["total"], json!(1));
    assert_eq!(
        page["data"]["data"][0]["key"],
        json!("notify.problem.raised")
    );
    assert_eq!(
        page["data"]["data"][0]["params"]["key"],
        json!(OVERDUE_PROBLEM)
    );
    http.stop().await.expect("the server stops");

    let done = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": overdue_note, "title": "renew the certificate", "kind": "memo" }),
        )
        .await;
    assert_eq!(data(&done)["kind"], json!("memo"));
    run_poll(&harness).await;
    let mut problems = vec![
        format!("{OVERDUE_PROBLEM} {a1} 1 {a1}"),
        format!("{OVERDUE_PROBLEM} {a2} 1 {a2}"),
    ];
    problems.sort();
    assert_eq!(open_problems(&harness).await, problems);
    let deleted = harness
        .call(
            Who::A1,
            "note.delete",
            json!({ "ids": [data(&harness.call(Who::A1, "note.list", json!({ "search": "water" })).await)["data"][0]["id"]] }),
        )
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    run_poll(&harness).await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{OVERDUE_PROBLEM} {a2} 1 {a2}")]
    );
    let cleared = harness
        .call(
            Who::A1,
            PROBLEM_LIST,
            json!({ "filters": [{ "key": "open", "op": "eq", "value": "false" }] }),
        )
        .await;
    assert_eq!(data(&cleared)["total"], json!(1));
    assert_eq!(data(&cleared)["data"][0]["count"], json!(2));
    let messages = harness
        .database()
        .rows(&format!(
            "SELECT key FROM notify_message WHERE principal_id = '{}' ORDER BY created_at, id",
            harness.principal(Who::A1).id
        ))
        .await;
    assert_eq!(
        messages,
        ["notify.problem.raised", "notify.problem.cleared"]
    );
    let http = harness.http().await;
    let boot = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(boot.json()["problems"], json!({ "open": 0, "critical": 0 }));
    let admin = http.get(Who::Admin, "/api/v1/boot").await;
    assert_eq!(
        admin.json()["problems"],
        json!({ "open": 1, "critical": 0 })
    );
    assert_eq!(
        admin.json()["dashboard"],
        json!([
            { "kind": "stat", "func": "keel.widget.accounts" },
            { "kind": "list", "func": PROBLEM_LIST, "limit": 5 }
        ])
    );
    http.stop().await.expect("the server stops");
    let _ = serde_json::to_string(&reported).expect("raises serialise");
    harness.finish().await;
}
