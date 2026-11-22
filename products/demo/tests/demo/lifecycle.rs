use std::time::Duration;

use keel::prelude::*;
use keel::types::TaskState;
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Reply, Who, tokio};

const WAIT: Duration = Duration::from_secs(30);

async fn create(harness: &Harness, who: Who, title: &str) -> String {
    let reply = harness
        .call(who, "note.edit", json!({ "title": title }))
        .await;
    reply
        .data()
        .unwrap_or_else(|| panic!("note.edit failed: {:?}", reply.error()))["id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

fn refused(reply: &Reply) -> (String, Value) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    (
        error.key.clone(),
        Value::Object(error.params.clone().into_iter().collect()),
    )
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

async fn held_becomes(harness: &Harness, account: Id, held: bool) {
    let sql = format!("SELECT account_id FROM note_hold WHERE account_id = '{account}'");
    for _ in 0..600 {
        if rows(harness, &sql).await.is_empty() != held {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the hold of account {account} did not become {held}");
}

#[tokio::test]
async fn notes_of_a_suspended_account_refuse_edits_and_disappear_with_the_account() {
    let harness = crate::demo().boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let kept = create(&harness, Who::A1, "kept").await;
    let spare = create(&harness, Who::A1, "spare").await;
    create(&harness, Who::A2, "theirs").await;

    let suspended = harness
        .call(Who::Reseller, "keel.account.suspend", json!({ "id": a1 }))
        .await;
    assert_eq!(
        suspended.data().map(|data| data["state"].clone()),
        Some(json!("suspended")),
        "{:?}",
        suspended.error()
    );
    let edit = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": kept, "title": "renamed while suspended" }),
        )
        .await;
    assert_eq!(refused(&edit).0, "note.error.account_suspended");
    held_becomes(&harness, a1, true).await;
    let removal = harness
        .call(Who::Reseller, "note.delete", json!({ "ids": [kept] }))
        .await;
    let (key, params) = refused(&removal);
    assert_eq!(key, "note.error.account_suspended");
    assert_eq!(params["title"], json!("kept"));
    assert_eq!(params["state"], json!("suspended"));

    let resumed = harness
        .call(Who::Reseller, "keel.account.resume", json!({ "id": a1 }))
        .await;
    assert_eq!(
        resumed.data().map(|data| data["state"].clone()),
        Some(json!("active")),
        "{:?}",
        resumed.error()
    );
    held_becomes(&harness, a1, false).await;
    let removal = harness
        .call(Who::A1, "note.delete", json!({ "ids": [spare] }))
        .await;
    assert_eq!(
        removal.data(),
        Some(&json!({ "deleted": 1 })),
        "{:?}",
        removal.error()
    );
    let edit = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": kept, "title": "renamed after resume" }),
        )
        .await;
    assert!(edit.data().is_some(), "{:?}", edit.error());

    let queued = harness
        .call(Who::Reseller, "keel.account.delete", json!({ "id": a1 }))
        .await;
    let task: Id = queued
        .data()
        .unwrap_or_else(|| panic!("a task ref was expected: {:?}", queued.error()))["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid");
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(WAIT, drain.wait(task))
        .await
        .expect("the job ends in time")
        .expect("the state is read");
    let detail = harness
        .call(Who::Reseller, "task.get", json!({ "id": task }))
        .await;
    let detail = detail
        .data()
        .unwrap_or_else(|| panic!("task.get failed: {:?}", detail.error()))
        .clone();
    assert_eq!(state, TaskState::Done, "{detail}");
    assert_eq!(
        detail["result"],
        json!({ "id": a1.to_string(), "modules": ["note", "notify"] })
    );
    let log = detail["logTail"].to_string();
    assert!(log.contains("dropped 1 note(s)"), "{log}");
    assert!(log.contains("deleted note"), "{log}");
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT count(*)::text FROM note_item WHERE account_id = '{a1}'")
        )
        .await,
        ["0"]
    );
    assert_eq!(
        rows(&harness, "SELECT title FROM note_item").await,
        ["theirs"]
    );
    assert!(
        rows(
            &harness,
            &format!("SELECT id FROM keel_account WHERE id = '{a1}'")
        )
        .await
        .is_empty()
    );
    let gone = harness
        .call(Who::Reseller, "keel.account.get", json!({ "id": a1 }))
        .await;
    assert_eq!(refused(&gone).0, "keel.error.not_found");
    harness.finish().await;
}
