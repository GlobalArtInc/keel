use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use keel::prelude::*;
use keel::sdk::chrono_tz;
use keel::sdk::sea_orm::ConnectionTrait;
use keel::sdk::serde_json::{self, Map};
use keel::types::TaskState;
use keel_demo::MANIFEST;
use keel_demo::note::{GROUP_BATCH_THRESHOLD, LOCKED_PREFIX, MAX_LINE, MAX_PER_ACCOUNT};
use keel_system::tasks::api::Drain;
use keel_testkit::{CallOptions, ErrorCode, Fixture, Harness, Reply, Who, tokio};

struct NoteFixture {
    created: AtomicU64,
}

impl NoteFixture {
    fn new() -> Self {
        Self {
            created: AtomicU64::new(0),
        }
    }

    fn next(&self) -> u64 {
        self.created.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[async_trait]
impl Fixture for NoteFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                "note.edit",
                json!({ "title": format!("note {} of {account}", self.next()), "account": account }),
            )
            .await;
        id_of(&reply).to_string()
    }
}

struct TagFixture {
    inner: NoteFixture,
}

#[async_trait]
impl Fixture for TagFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let tag = format!("tag{}", self.inner.next());
        let reply = harness
            .call(
                Who::Owner,
                "note.edit",
                json!({ "title": format!("tagged {tag} of {account}"), "tags": [tag], "account": account }),
            )
            .await;
        id_of(&reply);
        tag
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
    let harness = crate::demo().boot().await;
    assert_eq!(harness.registry().product_ids(), ["demo"]);
    assert_eq!(
        harness.modules(),
        crate::loaded(&["keel", "cron", "notify", "task", "note", "testkit"])
    );

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
        note["accountId"],
        json!(harness.fixtures().accounts.a1.to_string())
    );
    assert_eq!(note["createdAt"], json!(harness.clock().now()));
    assert_eq!(note["kind"], json!("memo"));
    assert_eq!(note["tags"], json!([]));
    assert_eq!(note["due"], Value::Null);
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
    assert_eq!(renamed["createdAt"], note["createdAt"]);
    assert_eq!(renamed["updatedAt"], json!(harness.clock().now()));
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_invariants_hold_for_notes() {
    let harness = crate::demo().boot().await;
    let notes = NoteFixture::new();
    harness
        .assert_reseller_sees_only_subtree("note.list", &notes)
        .await;
    for function in ["note.get", "note.edit", "note.delete"] {
        harness
            .assert_user_cannot_edit_foreign_row(function, &notes)
            .await;
    }
    let tags = TagFixture {
        inner: NoteFixture::new(),
    };
    harness
        .assert_reseller_sees_only_subtree("note.tag.list", &tags)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn note_saved_names_the_notes_account_as_its_subject() {
    let harness = crate::demo().boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let created = harness
        .call(
            Who::Owner,
            "note.edit",
            json!({ "title": "owned by a1", "account": a1 }),
        )
        .await;
    let id = id_of(&created);
    let renamed = harness
        .call(
            Who::Owner,
            "note.edit",
            json!({ "id": id, "title": "renamed by the owner" }),
        )
        .await;
    assert_eq!(id_of(&renamed), id);
    let subjects: Vec<Value> = harness
        .database()
        .rows("SELECT json_build_object('type', type, 'subject', subject_json)::text FROM keel_outbox ORDER BY seq")
        .await
        .iter()
        .map(|row| serde_json::from_str(row).expect("json"))
        .collect();
    let expected = json!({
        "type": "note.saved",
        "subject": { "kind": "note", "id": id, "accountId": a1 }
    });
    assert_eq!(subjects, [expected.clone(), expected]);
    harness.finish().await;
}

#[tokio::test]
async fn note_list_searches_and_sorts_and_note_delete_removes_a_batch_unless_a_note_is_locked() {
    let harness = crate::demo().boot().await;
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
    let by_tags = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "filters": [{ "key": "tags", "value": "x" }] }),
        )
        .await;
    assert_eq!(
        field_error(&by_tags, "filters[0].key").key.as_deref(),
        Some("keel.error.list.not_filterable")
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

fn note_rows(page: &Value) -> Vec<(String, String, Value, Value)> {
    let mut rows: Vec<(String, String, Value, Value)> = page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                row["title"].as_str().expect("a title").to_owned(),
                row["kind"].as_str().expect("a kind").to_owned(),
                row["due"].clone(),
                row["tags"].clone(),
            )
        })
        .collect();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

#[tokio::test]
async fn note_edit_as_a_group_sets_kind_due_and_tags_of_the_selected_notes_and_becomes_a_task_above_ten()
 {
    let harness = crate::demo().boot().await;
    let first = create(&harness, Who::A1, "first memo").await;
    let second = create(&harness, Who::A1, "second memo").await;
    let untouched = create(&harness, Who::A1, "untouched").await;
    let reply = harness
        .call(
            Who::A1,
            "note.edit",
            json!({
                "ids": [first, second],
                "fields": { "kind": "todo", "due": "2026-10-01", "tags": ["sprint"] },
                "mask": ["kind", "due", "tags"]
            }),
        )
        .await;
    assert_eq!(
        reply.data(),
        Some(&json!({ "updated": 2 })),
        "{:?}",
        reply.error()
    );
    let page = harness.call(Who::A1, "note.list", json!({})).await;
    assert_eq!(
        note_rows(page.data().expect("a page")),
        [
            (
                "first memo".to_owned(),
                "todo".to_owned(),
                json!("2026-10-01"),
                json!(["sprint"])
            ),
            (
                "second memo".to_owned(),
                "todo".to_owned(),
                json!("2026-10-01"),
                json!(["sprint"])
            ),
            (
                "untouched".to_owned(),
                "memo".to_owned(),
                Value::Null,
                json!([])
            ),
        ]
    );

    let without_due = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "ids": [untouched], "fields": { "kind": "todo" }, "mask": ["kind"] }),
        )
        .await;
    assert_eq!(
        field_error(&without_due, "due").key.as_deref(),
        Some("keel.schema.required")
    );
    let titled = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "ids": [untouched], "fields": { "title": "renamed" }, "mask": ["title"] }),
        )
        .await;
    assert_eq!(
        field_error(&titled, "mask[0]").key.as_deref(),
        Some("keel.error.multi_field")
    );

    let mut ids = vec![first, second, untouched];
    for index in ids.len()..GROUP_BATCH_THRESHOLD as usize + 1 {
        ids.push(create(&harness, Who::A1, &format!("memo {index}")).await);
    }
    let reply = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "ids": ids, "fields": { "tags": ["bulk"] }, "mask": ["tags"] }),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("a task ref was expected: {:?}", reply.error()));
    let task: Id = data["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid");
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    let state = tokio::time::timeout(std::time::Duration::from_secs(30), drain.wait(task))
        .await
        .expect("the job ends in time")
        .expect("the state is read");
    assert_eq!(state, TaskState::Done);
    let detail = harness
        .call(Who::A1, "task.get", json!({ "id": task }))
        .await;
    let detail = detail
        .data()
        .unwrap_or_else(|| panic!("task.get failed: {:?}", detail.error()));
    assert_eq!(detail["function"], json!("note.edit"));
    assert_eq!(detail["progress"], json!("11/11"));
    assert_eq!(
        detail["result"],
        json!([{ "updated": 10 }, { "updated": 1 }])
    );
    let page = harness.call(Who::A1, "note.list", json!({})).await;
    let rows = note_rows(page.data().expect("a page"));
    assert_eq!(rows.len(), 11);
    assert!(rows.iter().all(|(_, _, _, tags)| *tags == json!(["bulk"])));
    harness.finish().await;
}

#[tokio::test]
async fn note_edit_refuses_a_note_above_the_account_limit_inside_the_transaction() {
    let harness = crate::demo().boot().await;
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
    let harness = crate::demo().boot().await;
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
    let harness = crate::demo().boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors_of(level, snapshots_dir(), "note");
        let expected: &[&str] = if level < Level::User {
            &[]
        } else {
            &[
                "note.delete",
                "note.edit",
                "note.export",
                "note.get",
                "note.import",
                "note.import.check",
                "note.list",
                "note.recompute",
                "note.tag.list",
            ]
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
    let harness = crate::demo().boot().await;
    let registry = harness.registry();
    for key in [
        MANIFEST.name,
        "menu.demo",
        "note.error.locked",
        "note.error.limit",
        "note.error.exists",
        "note.error.line_too_long",
        "note.error.empty",
        "note.tag.list.title",
        "note.tag.column.tag",
        "note.tag.column.count",
        "note.import.title",
        "note.page.source",
        "note.page.options",
        "note.kind.memo",
        "note.kind.todo",
        "note.column.accountId",
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
    assert!(en.len() >= 30, "{en:?}");
    assert_eq!(
        en, ru,
        "the en and ru dictionaries of the demo name different keys"
    );
    harness.finish().await;
}

#[tokio::test]
async fn note_list_answers_over_http_with_the_envelope_and_boot_carries_the_demo_brand_and_menu() {
    let harness = crate::demo().boot().await;
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
                 "brand": { "name": "product.demo.brand", "logo": "/brand/demo/logo.svg", "hue": 264, "chroma": 0.18 } }])
    );
    assert_eq!(boot["keel"]["api"], json!(1));
    assert_eq!(boot["brand"]["name"], json!("product.demo.brand"));
    assert_eq!(boot["brand"]["hue"], json!(264));
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

fn column_headers(lang: &str) -> [String; 6] {
    let messages: Value = serde_json::from_str(match lang {
        "en" => include_str!("../../i18n/note/en.json"),
        _ => include_str!("../../i18n/note/ru.json"),
    })
    .expect("a JSON dictionary");
    let header = |key: &str| {
        messages["note"]["column"][key]
            .as_str()
            .expect("a column label")
            .to_owned()
    };
    [
        header("title"),
        header("kind"),
        header("tags"),
        header("due"),
        header("updatedAt"),
        header("accountId"),
    ]
}

#[tokio::test]
async fn ctl_note_list_out_table_shows_translated_headers_in_en_and_ru() {
    let harness = crate::demo().boot().await;
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
        envelope["data"]["accountId"],
        json!(Id::ROOT_ACCOUNT.to_string())
    );
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}

fn field_error(reply: &Reply, path: &str) -> keel::types::FieldError {
    let error = reply.error().expect("an error envelope");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(reply.status(), 422, "{error:?}");
    error
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("no field error on {path}: {error:?}"))
        .clone()
}

#[tokio::test]
async fn note_edit_refuses_a_second_note_with_the_same_title_as_a_field_error_on_title() {
    let harness = crate::demo().boot().await;
    let first = create(&harness, Who::A1, "groceries").await;
    let second = harness
        .call(Who::A1, "note.edit", json!({ "title": "groceries" }))
        .await;
    let error = field_error(&second, "title");
    assert_eq!(error.code.as_deref(), Some("note.error.exists"));
    assert_eq!(error.key.as_deref(), Some("note.error.exists"));
    assert_eq!(error.params["title"], json!("groceries"));
    let elsewhere = harness
        .call(Who::A2, "note.edit", json!({ "title": "groceries" }))
        .await;
    assert!(elsewhere.data().is_some(), "{:?}", elsewhere.error());
    let kept = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": first, "title": "groceries", "body": "milk" }),
        )
        .await;
    assert_eq!(
        kept.data().expect("the own title stays")["body"],
        json!("milk")
    );
    let other = create(&harness, Who::A1, "chores").await;
    let renamed = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "id": other, "title": "groceries" }),
        )
        .await;
    assert_eq!(
        field_error(&renamed, "title").key.as_deref(),
        Some("note.error.exists")
    );
    let todo_without_due = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "call back", "kind": "todo" }),
        )
        .await;
    let error = field_error(&todo_without_due, "due");
    assert_eq!(error.key.as_deref(), Some("keel.schema.required"));
    let todo = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "call back", "kind": "todo", "due": "2026-09-20", "tags": ["phone", "work"] }),
        )
        .await;
    let todo = todo.data().expect("a todo with a due date is saved");
    assert_eq!(todo["kind"], json!("todo"));
    assert_eq!(todo["due"], json!("2026-09-20"));
    assert_eq!(todo["tags"], json!(["phone", "work"]));
    let by_due = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "filters": [{ "key": "due", "op": "lte", "value": "2026-09-20" }] }),
        )
        .await;
    assert_eq!(titles(by_due.data().expect("a page")), ["call back"]);
    let by_kind = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "filters": [{ "key": "kind", "value": "memo" }], "sort": { "key": "title", "dir": "asc" } }),
        )
        .await;
    assert_eq!(
        titles(by_kind.data().expect("a page")),
        ["chores", "groceries"]
    );
    let bad_tag = harness
        .call(
            Who::A1,
            "note.edit",
            json!({ "title": "tagged", "tags": ["ok", "2026"] }),
        )
        .await;
    let error = field_error(&bad_tag, "tags[1]");
    assert_eq!(error.code.as_deref(), Some("check.name"));
    assert_eq!(error.key.as_deref(), Some("keel.check.name"));
    assert_eq!(error.params["value"], json!("2026"));
    let (notes, _) = counts(&harness).await;
    assert_eq!(notes, "4");
    harness.finish().await;
}

async fn recompute(harness: &Harness, who: Who, tz: chrono_tz::Tz, params: Value) -> Value {
    let reply = harness
        .call_with(
            harness.principal(who),
            "note.recompute",
            params,
            CallOptions {
                tz: Some(tz),
                ..CallOptions::default()
            },
        )
        .await;
    reply
        .data()
        .unwrap_or_else(|| panic!("note.recompute failed: {:?}", reply.error()))
        .clone()
}

#[tokio::test]
async fn note_recompute_fills_due_a_week_ahead_when_kind_turns_todo_and_patches_nothing_otherwise()
{
    let harness = crate::demo()
        .clock("2026-09-13T20:00:00Z".parse().expect("an instant"))
        .boot()
        .await;
    let turned_todo =
        json!({ "values": { "title": "", "kind": "todo", "due": null }, "changed": "kind" });
    assert_eq!(
        recompute(
            &harness,
            Who::A1,
            chrono_tz::Asia::Tokyo,
            turned_todo.clone()
        )
        .await,
        json!({ "values": { "due": "2026-09-21" } })
    );
    assert_eq!(
        recompute(&harness, Who::A1, chrono_tz::UTC, turned_todo.clone()).await,
        json!({ "values": { "due": "2026-09-20" } })
    );
    assert_eq!(
        recompute(
            &harness,
            Who::A1,
            chrono_tz::UTC,
            json!({ "values": { "kind": "todo" }, "changed": "kind" })
        )
        .await,
        json!({ "values": { "due": "2026-09-20" } })
    );
    for unchanged in [
        json!({ "values": { "kind": "todo", "due": "2026-10-01" }, "changed": "kind" }),
        json!({ "values": { "kind": "memo", "due": null }, "changed": "kind" }),
        json!({ "values": { "kind": "todo", "due": null }, "changed": "title" }),
        json!({ "values": { "kind": "later", "due": null }, "changed": "kind" }),
    ] {
        assert_eq!(
            recompute(&harness, Who::A1, chrono_tz::UTC, unchanged.clone()).await,
            json!({ "values": {} }),
            "{unchanged}"
        );
    }
    let malformed = harness
        .call(
            Who::A1,
            "note.recompute",
            json!({ "values": [], "changed": "kind" }),
        )
        .await;
    assert_eq!(malformed.status(), 422, "{:?}", malformed.envelope);
    let guest = harness
        .call(Who::Guest, "note.recompute", turned_todo)
        .await;
    assert_eq!(guest.status(), 401);
    let missing = harness
        .call_as(Level::User, "note.recompute", json!({}))
        .await;
    assert_eq!(missing.status(), 422, "{:?}", missing.envelope);
    harness.finish().await;
}

#[tokio::test]
async fn note_import_checks_the_source_page_and_creates_one_note_per_line_inside_the_limit() {
    let harness = crate::demo().boot().await;
    let empty = harness
        .call(Who::A1, "note.import.check", json!({ "text": "\n  \n" }))
        .await;
    assert_eq!(
        field_error(&empty, "text").key.as_deref(),
        Some("note.error.empty")
    );
    let missing = harness.call(Who::A1, "note.import.check", json!({})).await;
    assert_eq!(
        field_error(&missing, "text").key.as_deref(),
        Some("note.error.empty")
    );
    let long = harness
        .call(
            Who::A1,
            "note.import.check",
            json!({ "text": format!("fine\n\n{}", "x".repeat(MAX_LINE + 1)) }),
        )
        .await;
    let error = field_error(&long, "text");
    assert_eq!(error.key.as_deref(), Some("note.error.line_too_long"));
    assert_eq!(error.params["line"], json!(3));
    assert_eq!(error.params["max"], json!(MAX_LINE));
    let checked = harness
        .call(
            Who::A1,
            "note.import.check",
            json!({ "text": " milk \n\neggs\r\nbread\n" }),
        )
        .await;
    assert_eq!(checked.data(), Some(&json!({ "lines": 3 })));

    let imported = harness
        .call(
            Who::A1,
            "note.import",
            json!({ "text": " milk \n\neggs\r\nbread\n", "kind": "todo", "tags": ["shopping"] }),
        )
        .await;
    let imported = imported
        .data()
        .unwrap_or_else(|| panic!("note.import failed: {:?}", imported.error()));
    assert_eq!(imported["imported"], json!(3));
    assert_eq!(imported["ids"].as_array().map(Vec::len), Some(3));
    let listed = harness
        .call(
            Who::A1,
            "note.list",
            json!({ "sort": { "key": "title", "dir": "asc" } }),
        )
        .await;
    let page = listed.data().expect("a page");
    assert_eq!(titles(page), ["bread", "eggs", "milk"]);
    for row in page["data"].as_array().expect("rows") {
        assert_eq!(row["kind"], json!("todo"));
        assert_eq!(row["tags"], json!(["shopping"]));
        assert_eq!(row["due"], Value::Null);
        assert_eq!(row.get("accountId"), None, "{row}");
    }
    let a1 = harness.fixtures().accounts.a1.to_string();
    let scoped = harness
        .call(
            Who::Reseller,
            "note.list",
            json!({ "filters": [{ "key": "accountId", "value": a1 }, { "key": "kind", "value": "todo" }] }),
        )
        .await;
    let rows = scoped.data().expect("a page")["data"]
        .as_array()
        .expect("rows")
        .clone();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter().all(|row| row["accountId"] == json!(a1)),
        "{rows:?}"
    );
    let events = harness
        .database()
        .rows("SELECT type FROM keel_outbox WHERE type = 'note.imported'")
        .await;
    assert_eq!(events, ["note.imported"]);

    let too_long = harness
        .call(
            Who::A1,
            "note.import",
            json!({ "text": "x".repeat(MAX_LINE + 1) }),
        )
        .await;
    assert_eq!(
        field_error(&too_long, "text").key.as_deref(),
        Some("note.error.line_too_long")
    );
    let bad_tag = harness
        .call(
            Who::A1,
            "note.import",
            json!({ "text": "one", "tags": ["1st"] }),
        )
        .await;
    assert_eq!(
        field_error(&bad_tag, "tags[0]").code.as_deref(),
        Some("check.name")
    );
    let room = MAX_PER_ACCOUNT - 3;
    let over: Vec<String> = (0..=room).map(|index| format!("line {index}")).collect();
    let refused = harness
        .call(Who::A1, "note.import", json!({ "text": over.join("\n") }))
        .await;
    let error = refused.error().expect("the batch passes the limit");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "note.error.limit");
    assert_eq!(error.params["max"], json!(MAX_PER_ACCOUNT));
    let (notes, _) = counts(&harness).await;
    assert_eq!(notes, "3");
    let filled = harness
        .call(
            Who::A1,
            "note.import",
            json!({ "text": over[..room as usize].join("\n") }),
        )
        .await;
    assert_eq!(
        filled.data().map(|data| data["imported"].clone()),
        Some(json!(room))
    );
    let (notes, _) = counts(&harness).await;
    assert_eq!(notes, MAX_PER_ACCOUNT.to_string());

    let foreign = harness
        .call(
            Who::A1,
            "note.import",
            json!({ "text": "theirs", "account": harness.fixtures().accounts.a2 }),
        )
        .await;
    assert_eq!(
        foreign.error().map(|error| error.code),
        Some(ErrorCode::ForbiddenField)
    );
    let by_reseller = harness
        .call(
            Who::Reseller,
            "note.import",
            json!({ "text": "for a2", "account": harness.fixtures().accounts.a2 }),
        )
        .await;
    assert_eq!(
        by_reseller.data().map(|data| data["imported"].clone()),
        Some(json!(1))
    );
    let a2 = harness.call(Who::A2, "note.list", json!({})).await;
    assert_eq!(titles(a2.data().expect("a page")), ["for a2"]);
    let outside = harness
        .call(
            Who::Reseller,
            "note.import",
            json!({ "text": "not yours", "account": harness.fixtures().accounts.unrelated }),
        )
        .await;
    assert_eq!(
        field_error(&outside, "account").key.as_deref(),
        Some("keel.error.account_not_found")
    );
    harness.finish().await;
}

fn tag_rows(page: &Value) -> Vec<(String, i64)> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                row["tag"].as_str().expect("a tag").to_owned(),
                row["count"].as_i64().expect("a count"),
            )
        })
        .collect()
}

#[tokio::test]
async fn note_tag_list_counts_tags_within_the_callers_scope_with_a_sum_total() {
    let harness = crate::demo().boot().await;
    for (who, title, tags) in [
        (Who::A1, "a", vec!["work", "urgent"]),
        (Who::A1, "b", vec!["work"]),
        (Who::A1, "c", vec!["home", "work"]),
        (Who::A1, "d", vec![]),
        (Who::A2, "e", vec!["work", "travel"]),
        (Who::Unrelated, "f", vec!["work", "secret"]),
    ] {
        let saved = harness
            .call(who, "note.edit", json!({ "title": title, "tags": tags }))
            .await;
        assert!(saved.data().is_some(), "{who} {title}: {:?}", saved.error());
    }
    let mine = harness.call(Who::A1, "note.tag.list", json!({})).await;
    let page = mine.data().expect("a page");
    assert_eq!(
        tag_rows(page),
        [("work".into(), 3), ("home".into(), 1), ("urgent".into(), 1)]
    );
    assert_eq!(page["total"], json!(3));
    assert_eq!(page["totals"], json!({ "count": 5 }));

    let subtree = harness
        .call(Who::Reseller, "note.tag.list", json!({}))
        .await;
    let page = subtree.data().expect("a page");
    assert_eq!(
        tag_rows(page),
        [
            ("work".into(), 4),
            ("home".into(), 1),
            ("travel".into(), 1),
            ("urgent".into(), 1)
        ]
    );
    assert_eq!(page["totals"], json!({ "count": 7 }));
    let everything = harness.call(Who::Owner, "note.tag.list", json!({})).await;
    let page = everything.data().expect("a page");
    assert_eq!(tag_rows(page)[0], ("work".into(), 5));
    assert_eq!(page["total"], json!(5));
    assert_eq!(page["totals"], json!({ "count": 9 }));
    let nothing = harness.call(Who::Guest, "note.tag.list", json!({})).await;
    assert_eq!(nothing.status(), 401);

    let searched = harness
        .call(Who::A1, "note.tag.list", json!({ "search": "R" }))
        .await;
    assert_eq!(
        tag_rows(searched.data().expect("a page")),
        [("work".into(), 3), ("urgent".into(), 1)]
    );
    let by_tag = harness
        .call(
            Who::A1,
            "note.tag.list",
            json!({ "sort": { "key": "tag", "dir": "desc" } }),
        )
        .await;
    assert_eq!(
        tag_rows(by_tag.data().expect("a page")),
        [("work".into(), 3), ("urgent".into(), 1), ("home".into(), 1)]
    );
    let paged = harness
        .call(
            Who::A1,
            "note.tag.list",
            json!({ "page": 2, "pageSize": 2 }),
        )
        .await;
    let page = paged.data().expect("a page");
    assert_eq!(tag_rows(page), [("urgent".into(), 1)]);
    assert_eq!(page["total"], json!(3));
    assert_eq!(page["totals"], json!({ "count": 1 }));
    let popular = harness
        .call(
            Who::Reseller,
            "note.tag.list",
            json!({ "filters": [{ "key": "count", "op": "gte", "value": "2" }] }),
        )
        .await;
    assert_eq!(
        tag_rows(popular.data().expect("a page")),
        [("work".into(), 4)]
    );
    let named = harness
        .call(
            Who::A1,
            "note.tag.list",
            json!({ "filters": [{ "key": "tag", "value": ["home", "urgent"] }] }),
        )
        .await;
    assert_eq!(
        tag_rows(named.data().expect("a page")),
        [("home".into(), 1), ("urgent".into(), 1)]
    );
    let vague = harness
        .call(
            Who::A1,
            "note.tag.list",
            json!({ "filters": [{ "key": "count", "value": "many" }] }),
        )
        .await;
    assert_eq!(
        field_error(&vague, "filters[0].value").key.as_deref(),
        Some("keel.error.list.filter_value")
    );

    let socket = harness.socket().await;
    let table = socket.ctl_table("note.tag.list", &[], "en").await;
    assert!(table.ok, "{}", table.text);
    let lines: Vec<Vec<&str>> = table
        .text
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert_eq!(lines[0], ["Tag", "Count"], "{}", table.text);
    assert_eq!(lines[1], ["work", "5"], "{}", table.text);
    assert_eq!(lines.len(), 6, "{}", table.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}
