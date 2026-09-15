use keel::prelude::*;
use keel_system::audit::api::Flush;
use keel_testkit::{Harness, Who, tokio};

async fn flush(harness: &Harness) {
    harness
        .registry()
        .services()
        .get::<Flush>()
        .expect("the audit module provides Flush")
        .flush()
        .await
        .expect("the worker answers the flush");
}

#[tokio::test]
async fn note_edit_records_the_id_as_subject_and_the_masked_result_as_after_while_token_create_records_neither()
 {
    let harness = crate::demo()
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::audit::MODULE)
        .boot()
        .await;
    assert_eq!(
        harness.modules(),
        [
            "keel", "audit", "auth", "cron", "notify", "task", "note", "testkit"
        ]
    );
    let created = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "first", "body": "draft" }),
        )
        .await;
    let note = created
        .data()
        .unwrap_or_else(|| panic!("note.edit: {:?}", created.error()))
        .clone();
    let id = note["id"].as_str().expect("an id").to_owned();
    let edited = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": id, "title": "second", "body": "final" }),
        )
        .await;
    let after = edited
        .data()
        .unwrap_or_else(|| panic!("note.edit: {:?}", edited.error()))
        .clone();
    let token = harness
        .call(Who::A1, "auth.token.create", json!({ "name": "api" }))
        .await;
    let plaintext = token
        .data()
        .unwrap_or_else(|| panic!("auth.token.create: {:?}", token.error()))["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned();
    let deleted = harness
        .call(Who::A1, "note.delete", json!({ "ids": [id] }))
        .await;
    assert!(deleted.data().is_some(), "{:?}", deleted.error());
    flush(&harness).await;

    let trail = harness
        .database()
        .rows(
            "SELECT line FROM (SELECT function || ' ' || coalesce(subject, '-') || ' ' || (after IS NOT NULL)::text AS line FROM audit_call WHERE principal_login = 'a1') AS trail ORDER BY line COLLATE \"C\"",
        )
        .await;
    assert_eq!(
        trail,
        [
            "auth.token.create - false".to_owned(),
            format!("note.delete [\"{id}\"] false"),
            "note.edit - true".to_owned(),
            format!("note.edit {id} true"),
        ]
    );
    let leaked = harness
        .database()
        .rows(&format!(
            "SELECT count(*)::text FROM audit_call WHERE params_redacted::text LIKE '%{plaintext}%' OR coalesce(after::text, '') LIKE '%{plaintext}%'"
        ))
        .await;
    assert_eq!(leaked, ["0"]);
    let listed = harness
        .call(
            Who::A1,
            "audit.list",
            json!({ "filters": [{ "key": "function", "op": "eq", "value": "note.edit" }] }),
        )
        .await;
    let page = listed
        .data()
        .unwrap_or_else(|| panic!("audit.list: {:?}", listed.error()));
    let recorded: Vec<(Value, Value)> = page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| (row["subject"].clone(), row["after"].clone()))
        .collect();
    assert_eq!(after["title"], json!("second"));
    assert!(recorded.contains(&(json!(id), after)), "{recorded:?}");
    assert!(recorded.contains(&(json!(null), note)), "{recorded:?}");
    harness.finish().await;
}
