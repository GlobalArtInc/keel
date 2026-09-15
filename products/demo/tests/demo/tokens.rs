use keel::prelude::*;
use keel_testkit::{Answer, Harness, TestHttp, Who, tokio};

const JSON: (&str, &str) = ("content-type", "application/json");

async fn issue(harness: &Harness, scope: Value) -> String {
    let reply = harness
        .call(
            Who::A1,
            "auth.token.create",
            json!({ "name": "editor", "scope": scope }),
        )
        .await;
    reply
        .data()
        .unwrap_or_else(|| panic!("auth.token.create failed: {:?}", reply.error()))["token"]
        .as_str()
        .expect("the plaintext")
        .to_owned()
}

async fn call(http: &TestHttp, func: &str, token: &str, body: Value) -> Answer {
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &[("authorization", &format!("Bearer {token}")), JSON],
        Some(&body.to_string()),
    )
    .await
}

#[tokio::test]
async fn a_token_scoped_to_note_edit_alone_edits_a_note_through_its_nested_invokes() {
    let harness = crate::demo()
        .module(&keel_system::auth::MODULE)
        .boot()
        .await;
    assert_eq!(
        harness.modules(),
        ["keel", "auth", "task", "note", "testkit"]
    );
    let http = harness.http().await;
    let token = issue(&harness, json!({ "functions": ["note.edit"] })).await;

    let saved = call(
        &http,
        "note.edit",
        &token,
        json!({ "title": "by token", "body": "through nested invokes" }),
    )
    .await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let note = saved.json();
    assert_eq!(note["data"]["title"], json!("by token"));
    assert_eq!(
        note["data"]["accountId"],
        json!(harness.fixtures().accounts.a1.to_string())
    );
    assert_eq!(note["meta"]["principal"]["login"], json!("a1"));
    let id = note["data"]["id"].as_str().expect("an id").to_owned();
    let renamed = call(
        &http,
        "note.edit",
        &token,
        json!({ "id": id, "title": "renamed by token" }),
    )
    .await;
    assert_eq!(renamed.status, 200, "{}", renamed.body);
    assert_eq!(renamed.json()["data"]["title"], json!("renamed by token"));

    for outside in ["note.get", "note.list", "keel.account.get"] {
        let refused = call(&http, outside, &token, json!({ "id": id })).await;
        assert_eq!(refused.status, 403, "{outside}: {}", refused.body);
        assert_eq!(
            refused.json()["error"]["key"],
            json!("keel.error.token_scope"),
            "{outside}"
        );
        assert_eq!(
            refused.json()["error"]["params"]["function"],
            json!(outside),
            "{outside}"
        );
    }
    let listed = harness.call(Who::A1, "note.list", json!({})).await;
    let titles: Vec<&str> = listed.data().expect("a page")["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().expect("a title"))
        .collect();
    assert_eq!(titles, ["renamed by token"]);

    let reader = issue(&harness, json!("read_only")).await;
    let read = call(&http, "note.get", &reader, json!({ "id": id })).await;
    assert_eq!(read.status, 200, "{}", read.body);
    let write = call(
        &http,
        "note.edit",
        &reader,
        json!({ "id": id, "title": "no" }),
    )
    .await;
    assert_eq!(write.status, 403, "{}", write.body);
    assert_eq!(
        write.json()["error"]["key"],
        json!("keel.error.token_read_only")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
