use std::path::{Path, PathBuf};

use keel::prelude::*;
use keel::sdk::sea_orm::ConnectionTrait;
use keel::sdk::serde_json::{self, Map};
use keel_demo::MANIFEST;
use keel_demo::note::{LOCKED_PREFIX, MAX_PER_ACCOUNT};
use keel_testkit::{ErrorCode, Fixture, Harness, Reply, Who, tokio};

struct NoteFixture;

#[async_trait]
impl Fixture for NoteFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                "note.edit",
                json!({ "title": format!("note of {account}"), "account": account }),
            )
            .await;
        id_of(&reply).to_string()
    }
}

fn id_of(reply: &Reply) -> Id {
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("note.edit failed: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn titles(page: &Value) -> Vec<&str> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().expect("a title"))
        .collect()
}

async fn create(harness: &Harness, who: Who, title: &str) -> Id {
    id_of(
        &harness
            .call(who, "note.edit", json!({ "title": title }))
            .await,
    )
}

async fn counts(harness: &Harness) -> (String, String) {
    let notes = harness
        .database()
        .rows("SELECT count(*)::text FROM note_item")
        .await;
    let events = harness
        .database()
        .rows("SELECT count(*)::text FROM keel_outbox")
        .await;
    (notes[0].clone(), events[0].clone())
}

#[tokio::test]
async fn the_demo_bundle_boots_with_the_note_module_and_notes_are_scoped_by_account() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    assert_eq!(harness.registry().product_ids(), ["demo"]);
    assert_eq!(harness.modules(), ["keel", "note", "testkit"]);

    let saved = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "mine", "body": "the first note" }),
        )
        .await;
    let note = saved.data().expect("the note is saved").clone();
    assert_eq!(note["title"], json!("mine"));
    assert_eq!(note["body"], json!("the first note"));
    assert_eq!(
        note["account_id"],
        json!(harness.fixtures().accounts.a1.to_string())
    );
    assert_eq!(note["created_at"], json!(harness.clock().now()));
    let id = note["id"].as_str().expect("an id").to_owned();
    let got = harness.call(Who::A1, "note.get", json!({ "id": id })).await;
    assert_eq!(got.data(), Some(&note));

    let mine = harness.call(Who::A1, "note.list", json!({})).await;
    assert_eq!(titles(mine.data().expect("a page")), ["mine"]);
    let a2 = harness.call(Who::A2, "note.list", json!({})).await;
    assert!(titles(a2.data().expect("a page")).is_empty());
    let reseller = harness.call(Who::Reseller, "note.list", json!({})).await;
    assert_eq!(titles(reseller.data().expect("a page")), ["mine"]);

    for who in [Who::A2, Who::Unrelated] {
        let foreign = harness
            .call(who, "note.edit", json!({ "id": id, "title": "taken" }))
            .await;
        assert_eq!(
            foreign.error().map(|error| error.code),
            Some(ErrorCode::NotFound),
            "{who}"
        );
        let peek = harness.call(who, "note.get", json!({ "id": id })).await;
        assert_eq!(peek.status(), 404, "{who}: {:?}", peek.envelope);
    }
    let account_by_user = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "x", "account": harness.fixtures().accounts.a2 }),
        )
        .await;
    assert_eq!(
        account_by_user.error().map(|error| error.code),
        Some(ErrorCode::ForbiddenField)
    );
    let guest = harness.call(Who::Guest, "note.list", json!({})).await;
    assert_eq!(guest.status(), 401);

    harness
        .clock()
        .advance(keel::sdk::chrono::Duration::minutes(1));
    let renamed = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": id, "title": "renamed" }),
        )
        .await;
    let renamed = renamed.data().expect("the note is renamed");
    assert_eq!(renamed["title"], json!("renamed"));
    assert_eq!(renamed["body"], json!(""));
    assert_eq!(renamed["created_at"], note["created_at"]);
    assert_eq!(renamed["updated_at"], json!(harness.clock().now()));
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_invariants_hold_for_notes() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    harness
        .assert_reseller_sees_only_subtree("note.list", &NoteFixture)
        .await;
    for function in ["note.get", "note.edit", "note.delete"] {
        harness
            .assert_user_cannot_edit_foreign_row(function, &NoteFixture)
            .await;
    }
    harness.finish().await;
}

#[tokio::test]
async fn note_list_searches_and_sorts_and_note_delete_removes_a_batch_unless_a_note_is_locked() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let alpha = create(&harness, Who::A1, "alpha").await;
    let beta = create(&harness, Who::A1, "beta").await;
    let locked = create(&harness, Who::A1, &format!("{LOCKED_PREFIX} gamma")).await;
    let foreign = create(&harness, Who::Unrelated, "delta").await;

    let searched = harness
        .call(Who::A1, "note.list", json!({ "search": "alp" }))
        .await;
    assert_eq!(titles(searched.data().expect("a page")), ["alpha"]);
    let sorted = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "sort": { "key": "title", "dir": "desc" } }),
        )
        .await;
    assert_eq!(
        titles(sorted.data().expect("a page")),
        ["locked gamma", "beta", "alpha"]
    );
    let by_body = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "sort": { "key": "body", "dir": "asc" } }),
        )
        .await;
    assert_eq!(
        by_body.error().map(|error| error.code),
        Some(ErrorCode::Validation)
    );

    let refused = harness
        .call(Who::A1, "note.delete", json!({ "ids": [alpha, locked] }))
        .await;
    let error = refused.error().expect("the locked note vetoes the batch");
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(error.fields[0].path, "ids");
    assert_eq!(error.fields[0].key.as_deref(), Some("note.error.locked"));
    assert_eq!(error.fields[0].params["title"], json!("locked gamma"));
    let mixed = harness
        .call(Who::A1, "note.delete", json!({ "ids": [alpha, foreign] }))
        .await;
    assert_eq!(
        mixed.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    let empty = harness
        .call(Who::A1, "note.delete", json!({ "ids": [] }))
        .await;
    assert_eq!(
        empty.error().map(|error| error.code),
        Some(ErrorCode::Validation)
    );
    let all = harness.call(Who::A1, "note.list", json!({})).await;
    assert_eq!(all.data().expect("a page")["total"], json!(3));

    let deleted = harness
        .call(Who::A1, "note.delete", json!({ "ids": [alpha, beta] }))
        .await;
    assert_eq!(deleted.data(), Some(&json!({ "deleted": 2 })));
    let left = harness.call(Who::A1, "note.list", json!({})).await;
    assert_eq!(titles(left.data().expect("a page")), ["locked gamma"]);
    let events = harness
        .database()
        .rows("SELECT module FROM keel_outbox WHERE type = 'note.deleted'")
        .await;
    assert_eq!(events, ["note", "note"]);
    harness.finish().await;
}

#[tokio::test]
async fn note_edit_refuses_a_note_above_the_account_limit_inside_the_transaction() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let mut first = None;
    for index in 0..MAX_PER_ACCOUNT {
        let id = create(&harness, Who::A1, &format!("note {index}")).await;
        first.get_or_insert(id);
    }
    let refused = harness
        .call(Who::A1, "note.edit", json!({ "title": "one too many" }))
        .await;
    let error = refused.error().expect("the limit is enforced");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "note.error.limit");
    assert_eq!(error.params["max"], json!(MAX_PER_ACCOUNT));
    let by_reseller = harness
        .call(
            Who::Reseller,
            "note.edit",
            json!({ "title": "for a1", "account": harness.fixtures().accounts.a1 }),
        )
        .await;
    assert_eq!(
        by_reseller.error().map(|error| error.key.as_str()),
        Some("note.error.limit")
    );
    let edited = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": first.expect("the first note"), "title": "still editable" }),
        )
        .await;
    assert_eq!(
        edited.data().expect("editing stays allowed")["title"],
        json!("still editable")
    );
    let elsewhere = harness
        .call(Who::A2, "note.edit", json!({ "title": "room" }))
        .await;
    assert!(elsewhere.data().is_some(), "{:?}", elsewhere.error());
    let (notes, _) = counts(&harness).await;
    assert_eq!(notes, (MAX_PER_ACCOUNT + 1).to_string());
    harness.finish().await;
}

#[tokio::test]
async fn a_nested_invoke_reuses_the_transaction_of_the_call_on_note_edit() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let saved = harness
        .call(Who::A1, "note.edit", json!({ "title": "read back" }))
        .await;
    let note = saved
        .data()
        .expect("note.get inside note.edit sees the uncommitted row");
    assert_eq!(note["title"], json!("read back"));
    assert_eq!(counts(&harness).await, ("1".to_owned(), "1".to_owned()));

    let a1 = harness.fixtures().accounts.a1.to_string();
    harness
        .db()
        .execute_unprepared(&format!(
            "UPDATE keel_account SET state = 'suspended' WHERE id = '{a1}'"
        ))
        .await
        .expect("the account is suspended");
    let refused = harness
        .call(Who::A1, "note.edit", json!({ "title": "while suspended" }))
        .await;
    let error = refused
        .error()
        .expect("a suspended account cannot write notes");
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.key, "note.error.account_suspended");
    assert_eq!(error.params["state"], json!("suspended"));
    let renamed = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": note["id"], "title": "renamed while suspended" }),
        )
        .await;
    assert_eq!(
        renamed.error().map(|error| error.key.as_str()),
        Some("note.error.account_suspended")
    );
    assert_eq!(counts(&harness).await, ("1".to_owned(), "1".to_owned()));
    let unchanged = harness
        .call(Who::A1, "note.get", json!({ "id": note["id"] }))
        .await;
    assert_eq!(unchanged.data(), Some(note));
    harness.finish().await;
}

#[tokio::test]
async fn the_note_descriptors_match_their_snapshots_per_level() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let expected: &[&str] = if level < Level::User {
            &[]
        } else {
            &["note.delete", "note.edit", "note.get", "note.list"]
        };
        assert_eq!(names, expected, "{level}");
    }
    harness.finish().await;
}

fn lookup<'a>(messages: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    let mut current = messages;
    let mut segments = key.split('.').peekable();
    while let Some(segment) = segments.next() {
        let value = current.get(segment)?;
        if segments.peek().is_none() {
            return Some(value);
        }
        current = value.as_object()?;
    }
    None
}

fn leaves(messages: &Map<String, Value>, prefix: &str, into: &mut Vec<String>) {
    for (key, value) in messages {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(nested) => leaves(nested, &path, into),
            _ => into.push(path),
        }
    }
}

#[tokio::test]
async fn the_ru_dictionaries_name_every_key_of_the_en_dictionaries_freeze_requires() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let registry = harness.registry();
    for key in [
        MANIFEST.name,
        "menu.demo",
        "note.error.locked",
        "note.error.limit",
    ] {
        for lang in ["en", "ru"] {
            let found = lookup(registry.i18n(lang).expect(lang), key);
            assert!(
                found.is_some_and(|text| text.as_str().is_some_and(|text| !text.is_empty())),
                "{lang}: {key} is not translated"
            );
        }
    }
    let mut en = Vec::new();
    let mut ru = Vec::new();
    for (lang, into) in [("en", &mut en), ("ru", &mut ru)] {
        leaves(registry.i18n(lang).expect(lang), "", into);
        into.retain(|key| {
            key.starts_with("note.") || key.starts_with("product.demo.") || key == "menu.demo"
        });
        into.sort();
    }
    assert!(en.len() >= 15, "{en:?}");
    assert_eq!(
        en, ru,
        "the en and ru dictionaries of the demo name different keys"
    );
    harness.finish().await;
}

#[tokio::test]
async fn note_list_answers_over_http_with_the_envelope_and_boot_carries_the_demo_brand_and_menu() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    let server = harness.http().await;
    let saved = server
        .post(
            Who::A1,
            "/api/v1/call/note.edit",
            &json!({ "title": "over http" }),
        )
        .await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let listed = server
        .post(
            Who::A1,
            "/api/v1/call/note.list",
            &json!({ "search": "http" }),
        )
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    let envelope = listed.json();
    assert_eq!(envelope["ok"], json!(true));
    assert_eq!(titles(&envelope["data"]), ["over http"]);
    assert_eq!(
        envelope["meta"]["principal"]["login"],
        json!(Who::A1.login())
    );
    assert!(
        listed.header("x-keel-request-id").is_some(),
        "{:?}",
        listed.headers
    );
    let guest = server
        .post(Who::Guest, "/api/v1/call/note.list", &json!({}))
        .await;
    assert_eq!(guest.status, 401, "{}", guest.body);
    assert_eq!(guest.json()["error"]["code"], json!("unauthenticated"));

    let boot = server.get(Who::A1, "/api/v1/boot").await.json();
    assert_eq!(
        boot["products"],
        json!([{ "id": "demo", "name": "product.demo.brand", "edition": null,
                 "brand": { "name": "product.demo.brand", "logo": "/brand/demo/logo.svg", "hue": 150, "chroma": 0.1 } }])
    );
    assert_eq!(boot["keel"]["api"], json!(1));
    assert_eq!(boot["brand"]["name"], json!("product.demo.brand"));
    assert_eq!(boot["brand"]["hue"], json!(150));
    assert_eq!(boot["menu"][0]["id"], json!("demo"));
    assert_eq!(boot["menu"][0]["label"], json!("menu.demo"));
    assert_eq!(boot["menu"][0]["items"][0]["path"], json!("demo/notes"));
    assert_eq!(boot["menu"][0]["items"][0]["func"], json!("note.list"));
    assert_eq!(
        boot["menu"][0]["items"][0]["label"],
        json!("note.list.title")
    );
    let described = server
        .get(Who::A1, "/api/v1/functions/note.edit")
        .await
        .json();
    assert_eq!(described["ui"]["pages"][1]["id"], json!("owner"));
    assert_eq!(described["ui"]["pages"][1].get("fields"), None);
    assert_eq!(
        described["ui"]["pages"][1]["show_if"],
        json!({ "ne": ["$principal.level", "user"] })
    );
    server.stop().await.expect("the server stops");
    harness.finish().await;
}

fn column_headers(lang: &str) -> [String; 2] {
    let messages: Value = serde_json::from_str(match lang {
        "en" => include_str!("../i18n/note/en.json"),
        _ => include_str!("../i18n/note/ru.json"),
    })
    .expect("a JSON dictionary");
    let header = |key: &str| {
        messages["note"]["column"][key]
            .as_str()
            .expect("a column label")
            .to_owned()
    };
    [header("title"), header("updated_at")]
}

#[tokio::test]
async fn ctl_note_list_out_table_shows_translated_headers_in_en_and_ru() {
    let harness = Harness::new().product(&MANIFEST).boot().await;
    create(&harness, Who::A1, "tabled").await;
    let socket = harness.socket().await;
    for lang in ["en", "ru"] {
        let output = socket.ctl_table("note.list", &[], lang).await;
        assert!(output.ok, "{}", output.text);
        let lines: Vec<&str> = output.text.lines().collect();
        assert_eq!(lines.len(), 2, "{}", output.text);
        assert_eq!(
            lines[0].split_whitespace().collect::<Vec<_>>(),
            column_headers(lang),
            "{lang}: {}",
            output.text
        );
        assert!(lines[1].starts_with("tabled"), "{lang}: {}", output.text);
    }
    let edited = socket.ctl_json("note.edit", &["title=x"], "en").await;
    assert!(edited.ok, "{}", edited.text);
    let envelope: Value = serde_json::from_str(&edited.text).expect("an envelope");
    assert_eq!(envelope["ok"], json!(true));
    assert_eq!(envelope["data"]["title"], json!("x"));
    assert_eq!(
        envelope["data"]["account_id"],
        json!(Id::ROOT_ACCOUNT.to_string())
    );
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
