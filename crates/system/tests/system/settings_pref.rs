use keel_sdk::prelude::*;
use keel_system::settings::{PREF_LANG, PREF_THEME, PREF_TZ};
use keel_system::{auth, settings};
use keel_testkit::{Answer, ErrorCode, Harness, Reply, TestHttp, Who, tokio};

const PASSWORD: &str = "correct-horse";

async fn boot() -> Harness {
    let harness = Harness::new()
        .module(&auth::MODULE)
        .module(&settings::MODULE)
        .boot()
        .await;
    let a1 = harness.principal(Who::A1).id;
    let set = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1, "password": PASSWORD }),
        )
        .await;
    assert!(set.data().is_some(), "{:?}", set.error());
    harness
}

async fn set(harness: &Harness, who: Who, key: &str, value: Value) -> Reply {
    harness
        .call(
            who,
            "settings.pref.set",
            json!({ "key": key, "value": value }),
        )
        .await
}

async fn get(harness: &Harness, who: Who, key: &str) -> Value {
    let reply = harness
        .call(who, "settings.pref.get", json!({ "key": key }))
        .await;
    reply
        .data()
        .unwrap_or_else(|| panic!("{who} {key}: {:?}", reply.error()))
        .clone()
}

fn field_error(reply: &Reply, path: &str, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), 422, "{error:?}");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    let field = error.fields.first().expect("a field error");
    assert_eq!(field.path, path, "{error:?}");
    assert_eq!(field.key.as_deref(), Some(key), "{error:?}");
}

async fn login(http: &TestHttp) -> String {
    let answer = http
        .request(
            "POST",
            "/api/v1/call/auth.login",
            &[("content-type", "application/json")],
            Some(&json!({ "login": "a1", "password": PASSWORD }).to_string()),
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer
        .session_cookie()
        .unwrap_or_else(|| panic!("no session cookie: {:?}", answer.headers))
}

async fn with_cookie(http: &TestHttp, path: &str, cookie: &str) -> Answer {
    http.request("GET", path, &[("cookie", cookie)], None).await
}

#[tokio::test]
async fn a1_sets_tz_and_lang_and_boot_and_the_catalogue_follow_while_a2_reads_nothing_of_it() {
    let harness = boot().await;
    let tz = set(&harness, Who::A1, PREF_TZ, json!("Asia/Tokyo")).await;
    assert_eq!(
        tz.data(),
        Some(&json!({ "key": "tz", "value": "Asia/Tokyo" })),
        "{:?}",
        tz.error()
    );
    let lang = set(&harness, Who::A1, PREF_LANG, json!("ru")).await;
    assert_eq!(lang.status(), 200, "{:?}", lang.error());
    let theme = set(&harness, Who::A1, PREF_THEME, json!("dark")).await;
    assert_eq!(theme.status(), 200, "{:?}", theme.error());
    let columns = set(
        &harness,
        Who::A1,
        "columns.keel.account.list",
        json!({ "order": ["kind", "id"], "hidden": ["state"] }),
    )
    .await;
    assert_eq!(columns.status(), 200, "{:?}", columns.error());
    assert_eq!(
        get(&harness, Who::A1, PREF_TZ).await,
        json!({ "key": "tz", "value": "Asia/Tokyo" })
    );
    assert_eq!(
        get(&harness, Who::A1, "columns.keel.account.list").await["value"]["order"],
        json!(["kind", "id"])
    );
    assert_eq!(
        get(&harness, Who::A2, PREF_TZ).await,
        json!({ "key": "tz", "value": null })
    );
    assert_eq!(
        get(&harness, Who::A2, PREF_LANG).await,
        json!({ "key": "lang", "value": null })
    );
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM settings_pref")
        .await;
    assert_eq!(rows, ["4"]);

    let http = harness.http().await;
    let cookie = login(&http).await;
    let booted = with_cookie(&http, "/api/v1/boot", &cookie).await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    let principal = &booted.json()["principal"];
    assert_eq!(principal["login"], json!("a1"));
    assert_eq!(principal["lang"], json!("ru"));
    assert_eq!(principal["tz"], json!("Asia/Tokyo"));
    let catalogue = with_cookie(&http, "/api/v1/i18n", &cookie).await;
    assert_eq!(catalogue.status, 200, "{}", catalogue.body);
    assert_eq!(catalogue.json()["data"]["lang"], json!("ru"));
    assert_eq!(
        catalogue.json()["data"]["messages"]["settings"]["list"]["title"],
        json!("Настройки")
    );
    let listed = http
        .request(
            "POST",
            "/api/v1/call/keel.account.list",
            &[("cookie", &cookie), ("content-type", "application/json")],
            Some("{}"),
        )
        .await;
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert_eq!(listed.json()["meta"]["principal"]["login"], json!("a1"));
    let a2 = http.get(Who::A2, "/api/v1/boot").await;
    assert_eq!(a2.status, 200, "{}", a2.body);
    assert_eq!(a2.json()["principal"]["lang"], json!("en"));
    assert_eq!(a2.json()["principal"]["tz"], json!(null));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn an_unknown_zone_an_unserved_language_and_a_foreign_key_are_refused_on_their_field() {
    let harness = boot().await;
    let zone = set(&harness, Who::A1, PREF_TZ, json!("Mars/Olympus")).await;
    field_error(&zone, "value", "settings.error.pref_tz");
    let number = set(&harness, Who::A1, PREF_TZ, json!(9)).await;
    field_error(&number, "value", "settings.error.pref_text");
    let language = set(&harness, Who::A1, PREF_LANG, json!("de")).await;
    field_error(&language, "value", "settings.error.pref_lang");
    for key in ["widgets", "columns.", "columns.Note.List", "tz.name"] {
        let foreign = set(&harness, Who::A1, key, json!("x")).await;
        field_error(&foreign, "key", "settings.error.pref_key");
        let unread = harness
            .call(Who::A1, "settings.pref.get", json!({ "key": key }))
            .await;
        field_error(&unread, "key", "settings.error.pref_key");
    }
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM settings_pref")
        .await;
    assert_eq!(rows, ["0"]);
    let guest = harness
        .call(Who::Guest, "settings.pref.get", json!({ "key": PREF_TZ }))
        .await;
    assert_eq!(guest.status(), 401, "{:?}", guest.error());
    let service = set(&harness, Who::Service, PREF_TZ, json!("UTC")).await;
    assert_eq!(service.status(), 403, "{:?}", service.error());
    harness.finish().await;
}

#[tokio::test]
async fn the_owner_has_no_preferences_and_is_refused_before_any_row_is_written() {
    let harness = boot().await;
    let written = set(&harness, Who::Owner, PREF_TZ, json!("UTC")).await;
    assert_eq!(written.status(), 403, "{:?}", written.error());
    let error = written.error().expect("a refusal");
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.key, "settings.error.pref_owner");
    let read = harness
        .call(Who::Owner, "settings.pref.get", json!({ "key": PREF_TZ }))
        .await;
    assert_eq!(read.status(), 403, "{:?}", read.error());
    assert_eq!(
        read.error().map(|error| error.key.clone()).as_deref(),
        Some("settings.error.pref_owner")
    );
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM settings_pref")
        .await;
    assert_eq!(rows, ["0"]);
    harness.finish().await;
}

#[tokio::test]
async fn columns_take_column_key_lists_within_four_kib_and_at_most_128_functions_per_principal() {
    let harness = boot().await;
    let key = "columns.keel.account.list";
    for value in [
        json!("kind,id"),
        json!(["kind", "id"]),
        json!({ "order": "kind" }),
        json!({ "order": [1, 2] }),
        json!({ "order": ["k".repeat(65)] }),
        json!({ "order": vec!["kind"; 65] }),
        json!({ "widths": { "kind": 120 } }),
        Value::Object(
            (0..64)
                .map(|n| (format!("group{n:02}"), json!(["c".repeat(64)])))
                .collect(),
        ),
    ] {
        let refused = set(&harness, Who::A1, key, value).await;
        field_error(&refused, "value", "settings.error.pref_columns");
    }
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM settings_pref")
        .await;
    assert_eq!(rows, ["0"]);
    for n in 0..128 {
        let kept = set(
            &harness,
            Who::A1,
            &format!("columns.demo.list{n}"),
            json!({ "order": ["id"] }),
        )
        .await;
        assert_eq!(kept.status(), 200, "{n}: {:?}", kept.error());
    }
    let more = set(&harness, Who::A1, key, json!({ "order": ["id"] })).await;
    assert_eq!(more.status(), 409, "{:?}", more.error());
    let error = more.error().expect("a refusal");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.key, "settings.error.pref_columns_many");
    assert_eq!(error.params["max"], json!(128));
    let again = set(
        &harness,
        Who::A1,
        "columns.demo.list7",
        json!({ "order": ["id"], "hidden": ["state"] }),
    )
    .await;
    assert_eq!(again.status(), 200, "{:?}", again.error());
    let theme = set(&harness, Who::A1, PREF_THEME, json!("dark")).await;
    assert_eq!(theme.status(), 200, "{:?}", theme.error());
    let a2 = set(&harness, Who::A2, key, json!({ "order": ["id"] })).await;
    assert_eq!(a2.status(), 200, "{:?}", a2.error());
    let rows = harness
        .database()
        .rows("SELECT count(*)::text FROM settings_pref")
        .await;
    assert_eq!(rows, ["130"]);
    harness.finish().await;
}
