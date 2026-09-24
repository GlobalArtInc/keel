use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::serde_json;
use keel::types::{Envelope, ErrorCode};
use keel::{Change, Config, Running, Runtime};
use keel_demo::MANIFEST;
use keel_demo::note::FUNCTIONS;
use keel_testkit::{TestDatabase, TestSocket, Who, tokio};
use tempfile::TempDir;

struct Site {
    database: TestDatabase,
    dir: TempDir,
    config: Config,
}

async fn site() -> Site {
    let database = TestDatabase::create().await;
    let mut config = Config::parse(None, |_| None).expect("the platform defaults");
    assert_eq!(config.socket.as_os_str(), "/run/keel/core.sock");
    assert_eq!(config.data_dir.as_os_str(), "/var/lib/keel");
    assert_eq!(
        config.database.expose(),
        "postgres:///keel?host=/run/postgresql"
    );
    let dir = tempfile::tempdir().expect("a temporary directory");
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    config.database = database.config().database.clone();
    Site {
        database,
        dir,
        config,
    }
}

impl Site {
    async fn finish(self) {
        drop(self.dir);
        self.database.finish().await;
    }

    async fn tables(&self) -> Vec<String> {
        self.database
            .rows("SELECT table_name::text FROM information_schema.tables WHERE table_name LIKE 'note%' ORDER BY table_name")
            .await
    }
}

async fn http(running: &Running, method: &str, path: &str, body: &str) -> (u16, Value) {
    let url = running.url(path);
    assert!(url.starts_with("https://"), "{url}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([reqwest::Certificate::from_pem(
            running.tls().certificate_pem().as_bytes(),
        )
        .expect("the certificate parses")])
        .timeout(Duration::from_secs(10))
        .build()
        .expect("a client");
    let method: reqwest::Method = method.parse().expect("a method");
    let response = client
        .request(method, url)
        .header("host", "keel.test")
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .expect("the listener answers");
    let status = response.status().as_u16();
    let text = response.text().await.expect("the body is read");
    (status, serde_json::from_str(&text).expect("a JSON body"))
}

#[tokio::test]
async fn keel_serve_with_demo_enabled_starts_without_a_config_file_and_answers_on_the_socket_and_over_http()
 {
    let site = site().await;
    let runtime = Runtime::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .product(&MANIFEST);
    assert_eq!(
        runtime
            .enable_product(&site.config, "demo")
            .await
            .expect("demo is carried"),
        Change::Enabled
    );
    let migrated = runtime.migrate(&site.config).await.expect("migrate runs");
    let migrated_ids: Vec<&str> = migrated.modules.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        migrated_ids,
        crate::loaded(&["keel", "cron", "notify", "task", "note"])
    );
    let note = migrated.modules.iter().find(|(id, _)| id == "note");
    assert_eq!(
        note.map(|(_, applied)| applied.len()),
        Some(3),
        "{migrated:?}"
    );
    assert_eq!(site.tables().await, ["note_hold", "note_item"]);
    assert_eq!(
        site.database
            .rows("SELECT count(*)::text FROM keel_migration_note")
            .await,
        ["3"]
    );

    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts");
    assert_eq!(running.products(), ["demo"]);
    assert_eq!(
        running.modules(),
        crate::loaded(&["keel", "cron", "notify", "task", "note"])
    );
    let socket = TestSocket::at(running.socket());
    let listed = socket.call("note.list", json!({})).await;
    let Envelope::Ok { data, meta } = listed else {
        panic!("expected an ok envelope: {listed:?}");
    };
    assert_eq!(data["total"], json!(0));
    assert_eq!(
        meta.principal.map(|principal| principal.login),
        Some(Who::Owner.login().to_owned())
    );
    let boot = socket.boot().await;
    let Envelope::Ok { data: boot, .. } = boot else {
        panic!("expected an ok envelope: {boot:?}");
    };
    assert_eq!(boot.products[0].id, "demo");
    assert_eq!(boot.products[0].brand.hue, 264);
    assert_eq!(boot.brand.name, "product.demo.brand");
    assert_eq!(boot.menu[0].id, "demo");

    let edited = socket.ctl_json("note.edit", &["title=x"], "en").await;
    assert!(edited.ok, "{}", edited.text);
    let envelope: Value = serde_json::from_str(&edited.text).expect("an envelope");
    assert_eq!(envelope["ok"], json!(true));
    assert_eq!(envelope["data"]["title"], json!("x"));
    assert_eq!(
        envelope["meta"]["principal"]["login"],
        json!(Who::Owner.login())
    );
    let table = socket.ctl_table("note.list", &[], "ru").await;
    assert!(table.ok, "{}", table.text);
    let ru: Value = serde_json::from_str(include_str!("../../i18n/note/ru.json")).expect("ru");
    let lines: Vec<&str> = table.text.lines().collect();
    let header = |key: &str| ru["note"]["column"][key].as_str().expect("a label");
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        [
            header("title"),
            header("kind"),
            header("tags"),
            header("due"),
            header("updatedAt"),
            header("accountId"),
        ],
        "{}",
        table.text
    );
    assert!(lines[1].starts_with("x "), "{}", table.text);

    let (status, body) = http(&running, "GET", "/readyz", "").await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = http(&running, "POST", "/api/v1/call/note.list", "{}").await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["error"]["code"], json!("unauthenticated"));
    assert!(body["error"]["requestId"].is_string(), "{body}");
    let (status, body) = http(&running, "GET", "/api/v1/boot", "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["products"][0]["id"], json!("demo"));
    assert_eq!(body["brand"]["name"], json!("product.demo.brand"));
    assert_eq!(body["principal"], Value::Null);
    running.stop().await.expect("the runtime stops");
    assert!(!site.config.socket.exists());
    site.finish().await;
}

#[tokio::test]
async fn with_demo_disabled_the_note_functions_are_unknown_and_no_note_table_is_created() {
    let site = site().await;
    let runtime = Runtime::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .product(&MANIFEST);
    let states = runtime
        .list_products(&site.config)
        .await
        .expect("the list reads the database");
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].id, "demo");
    assert!(!states[0].enabled);
    let migrated = runtime.migrate(&site.config).await.expect("migrate runs");
    assert_eq!(migrated.modules.len(), 4);
    assert_eq!(migrated.modules[0].0, "keel");
    assert_eq!(migrated.modules[1].0, "cron");
    assert_eq!(migrated.modules[2].0, "notify");
    assert_eq!(migrated.modules[3].0, "task");
    assert!(site.tables().await.is_empty());

    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts without the bundle");
    assert_eq!(running.products(), &[] as &[String]);
    assert_eq!(running.modules(), ["keel", "cron", "notify", "task"]);
    let socket = TestSocket::at(running.socket());
    for function in FUNCTIONS {
        let answer = socket.call(function, json!({})).await;
        let Envelope::Err { error } = answer else {
            panic!("{function} is known although demo is disabled: {answer:?}");
        };
        assert_eq!(error.code, ErrorCode::NotFound, "{function}");
        assert_eq!(error.key, "keel.error.function_not_found", "{function}");
    }
    let (status, body) = http(&running, "GET", "/api/v1/boot", "").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["products"], json!([]));
    assert_eq!(body["brand"]["name"], json!("keel.brand"));
    running.stop().await.expect("the runtime stops");
    assert!(site.tables().await.is_empty());
    assert_eq!(
        site.database
            .rows("SELECT id FROM keel_module ORDER BY id")
            .await,
        ["cron", "keel", "notify", "task"]
    );
    site.finish().await;
}
