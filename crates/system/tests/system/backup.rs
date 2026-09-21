use std::io::Read;
use std::path::Path;
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::BackupManifest;
use keel_sdk::types::TaskState;
use keel_system::backup::{CREATE, ERROR_NAME, ERROR_NOT_FOUND, LIST, MENU_PATH, MODULE, RESTORE};
use keel_system::tasks::api::Drain;
use keel_testkit::{Harness, Reply, Who, tokio};

const WAIT: Duration = Duration::from_secs(30);
const DOWNLOAD_ROUTE: &str = "/api/v1/x/task/download";

async fn boot() -> Harness {
    Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&MODULE)
        .boot()
        .await
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("the call failed: {:?}", reply.error()))
}

fn task_of(reply: &Reply) -> Id {
    data(reply)["task"]["id"]
        .as_str()
        .expect("a task id")
        .parse()
        .expect("a ulid")
}

async fn wait_final(harness: &Harness, id: Id) -> TaskState {
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    tokio::time::timeout(WAIT, drain.wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

async fn detail_of(harness: &Harness, id: Id) -> Value {
    let reply = harness
        .call(Who::Admin, "task.get", json!({ "id": id }))
        .await;
    data(&reply).clone()
}

async fn run(harness: &Harness, func: &str, params: Value) -> Value {
    let reply = harness.call(Who::Admin, func, params).await;
    let id = task_of(&reply);
    let state = wait_final(harness, id).await;
    let detail = detail_of(harness, id).await;
    assert_eq!(state, TaskState::Done, "{detail}");
    detail
}

fn manifest_of(archive: &Path) -> BackupManifest {
    let file = std::fs::File::open(archive).expect("the archive opens");
    let mut tar = tar::Archive::new(zstd::Decoder::new(file).expect("zstd"));
    for entry in tar.entries().expect("entries") {
        let mut entry = entry.expect("an entry");
        if entry.path().expect("a path") == Path::new("manifest.json") {
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("the manifest reads");
            return serde_json::from_str(&text).expect("the manifest parses");
        }
    }
    panic!("no manifest in {}", archive.display());
}

fn write_archive(directory: &Path, name: &str, manifest: &BackupManifest) {
    std::fs::create_dir_all(directory).expect("the backup directory");
    let file = std::fs::File::create(directory.join(name)).expect("the archive is created");
    let encoder = zstd::Encoder::new(file, 1).expect("an encoder");
    let mut builder = tar::Builder::new(encoder);
    let text = serde_json::to_vec(manifest).expect("json");
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "manifest.json", text.as_slice())
        .expect("the manifest is appended");
    builder
        .into_inner()
        .expect("the tar ends")
        .finish()
        .expect("the stream ends");
}

fn foreign_manifest(keel: &str, product: &str) -> BackupManifest {
    BackupManifest {
        keel: keel.to_owned(),
        at: "2026-09-21T10:15:30Z".parse().expect("an instant"),
        instance: None,
        schema: "keel".into(),
        products: vec![keel_sdk::system::BackupProduct {
            id: product.to_owned(),
            version: "0.1.0".into(),
            edition: None,
        }],
        modules: Vec::new(),
        tables: Vec::new(),
        sources: Vec::new(),
        config: Vec::new(),
        data: Vec::new(),
        secret_key: false,
    }
}

#[tokio::test]
async fn backup_create_runs_as_a_task_that_writes_the_archive_under_backups_and_offers_it_for_download()
 {
    let harness = boot().await;
    let backups = harness.data_dir().join("backups");
    let empty = harness.call(Who::Admin, LIST, json!({})).await;
    assert_eq!(data(&empty)["total"], json!(0));
    assert_eq!(data(&empty)["data"], json!([]));

    let detail = run(&harness, CREATE, json!({})).await;
    assert_eq!(detail["function"], json!(CREATE));
    assert_eq!(detail["class"], json!("backup"));
    assert_eq!(detail["progress"], json!("2/2"));
    let result = &detail["result"];
    let file = result["file"].as_str().expect("a file name").to_owned();
    assert!(
        file.starts_with(&format!("keel-{}-", keel_system::VERSION)) && file.ends_with(".tar.zst"),
        "{file}"
    );
    let archive = backups.join(&file);
    assert_eq!(result["archive"], json!(archive.display().to_string()));
    assert_eq!(
        result["bytes"],
        json!(std::fs::metadata(&archive).expect("the archive").len())
    );
    assert_eq!(result["keel"], json!(keel_system::VERSION));
    assert_eq!(result["products"], json!([]));
    assert!(
        result["tables"].as_u64().is_some_and(|tables| tables > 5),
        "{result}"
    );
    assert_eq!(result["sources"], json!([]));
    let id: Id = detail["id"].as_str().expect("id").parse().expect("ulid");
    assert_eq!(
        result["download"],
        json!({ "function": "task.download", "params": { "id": id.to_string() } })
    );
    let log = detail["logTail"].as_array().expect("a log").clone();
    assert!(
        log.iter().any(|line| line
            .as_str()
            .is_some_and(|line| line.contains("table(s) of keel"))),
        "{log:?}"
    );

    let manifest = manifest_of(&archive);
    assert_eq!(manifest.keel, keel_system::VERSION);
    assert!(manifest.tables.iter().any(|table| table.name == "task_job"));
    assert!(manifest.modules.iter().any(|module| module.id == "backup"));
    assert!(
        manifest
            .modules
            .iter()
            .any(|module| module.id == "task" && module.applied >= 1)
    );
    assert!(!manifest.secret_key);
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT file_name || ' ' || file_type FROM task_job WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(stored, [format!("{file} application/zstd")]);
    let output = harness.data_dir().join("tasks").join(format!("{id}.out"));
    assert_eq!(
        std::fs::read(&output).expect("the task output"),
        std::fs::read(&archive).expect("the archive")
    );

    let http = harness.http().await;
    let got = http
        .get(Who::Admin, &format!("{DOWNLOAD_ROUTE}?id={id}"))
        .await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(got.header("content-type"), Some("application/zstd"));
    assert_eq!(
        got.header("content-disposition"),
        Some(format!("attachment; filename=\"{file}\"").as_str())
    );
    http.stop().await.expect("the server stops");

    let listed = harness.call(Who::Admin, LIST, json!({})).await;
    let page = data(&listed);
    assert_eq!(page["total"], json!(1));
    let row = &page["data"][0];
    assert_eq!(row["file"], json!(file));
    assert_eq!(row["keel"], json!(keel_system::VERSION));
    assert_eq!(row["products"], json!(""));
    assert_eq!(row["secretKey"], json!(false));
    assert_eq!(row["bytes"], result["bytes"]);
    assert!(
        row["tables"].as_u64().is_some_and(|tables| tables > 5),
        "{row}"
    );

    std::fs::write(backups.join("notes.txt"), "not an archive").expect("a stray file");
    std::fs::write(backups.join("broken.tar.zst"), "not zstd").expect("a broken archive");
    std::fs::create_dir_all(backups.join("pre-update-0.1.0-20260921T101530Z")).expect("a dump");
    let reply = harness.call(Who::Admin, CREATE, json!({})).await;
    let same_second = task_of(&reply);
    assert_eq!(wait_final(&harness, same_second).await, TaskState::Failed);
    let refused = detail_of(&harness, same_second).await;
    assert_eq!(
        refused["error"]["key"],
        json!("keel.error.backup_exists"),
        "an archive is never overwritten: {refused}"
    );
    harness
        .clock()
        .advance(keel_sdk::chrono::Duration::seconds(1));
    let second = run(&harness, CREATE, json!({})).await;
    let second_file = second["result"]["file"]
        .as_str()
        .expect("a file")
        .to_owned();
    let listed = harness
        .call(
            Who::Admin,
            LIST,
            json!({ "sort": { "key": "file", "dir": "asc" }, "pageSize": 1 }),
        )
        .await;
    let page = data(&listed);
    assert_eq!(page["total"], json!(2));
    assert_eq!(page["data"].as_array().map(Vec::len), Some(1));
    let searched = harness
        .call(
            Who::Admin,
            LIST,
            json!({ "search": second_file.trim_end_matches(".tar.zst") }),
        )
        .await;
    assert_ne!(second_file, file);
    assert_eq!(data(&searched)["total"], json!(1));
    let filtered = harness
        .call(
            Who::Admin,
            LIST,
            json!({ "filters": [{ "key": "keel", "op": "ne", "value": keel_system::VERSION }] }),
        )
        .await;
    assert_eq!(data(&filtered)["total"], json!(0));
    let refused = harness
        .call(
            Who::Admin,
            LIST,
            json!({ "sort": { "key": "products", "dir": "asc" } }),
        )
        .await;
    assert_eq!(
        refused.error().map(|error| error.code.as_str()),
        Some("validation")
    );
    let user = harness.call(Who::A1, LIST, json!({})).await;
    assert_eq!(
        user.error().map(|error| error.code.as_str()),
        Some("forbidden")
    );
    let user = harness.call(Who::A1, CREATE, json!({})).await;
    assert_eq!(
        user.error().map(|error| error.code.as_str()),
        Some("forbidden")
    );
    harness.finish().await;
}

#[tokio::test]
async fn backup_restore_checks_the_named_archive_and_writes_the_restore_request() {
    let harness = boot().await;
    let backups = harness.data_dir().join("backups");
    let created = run(&harness, CREATE, json!({})).await;
    let file = created["result"]["file"]
        .as_str()
        .expect("a file")
        .to_owned();

    let detail = run(&harness, RESTORE, json!({ "archive": file })).await;
    assert_eq!(detail["progress"], json!("2/2"));
    let result = &detail["result"];
    assert_eq!(
        result["archive"],
        json!(backups.join(&file).display().to_string())
    );
    assert_eq!(result["keel"], json!(keel_system::VERSION));
    assert_eq!(result["products"], json!([]));
    assert_eq!(result["secretKey"], json!(false));
    assert_eq!(result["requestedBy"], json!(Who::Admin.login()));
    let request_path = harness.data_dir().join("restore.request");
    assert_eq!(result["request"], json!(request_path.display().to_string()));
    let request: Value = serde_json::from_str(
        &std::fs::read_to_string(&request_path).expect("the request is written"),
    )
    .expect("json");
    assert_eq!(
        request,
        json!({ "archive": backups.join(&file).display().to_string(), "requested_by": Who::Admin.login() })
    );
    let log = detail["logTail"].as_array().expect("a log").clone();
    assert!(
        log.iter().any(|line| line
            .as_str()
            .is_some_and(|line| line.contains("restore request is written"))),
        "{log:?}"
    );

    let bad_name = harness
        .call(
            Who::Admin,
            RESTORE,
            json!({ "archive": "../etc/passwd.tar.zst" }),
        )
        .await;
    let error = bad_name
        .error()
        .expect("a refusal before the task is spawned");
    assert_eq!(error.code.as_str(), "validation");
    assert_eq!(error.fields[0].path, "archive");
    assert_eq!(error.fields[0].key.as_deref(), Some("keel.schema.pattern"));
    let hidden = harness
        .call(Who::Admin, RESTORE, json!({ "archive": ".hidden.tar.zst" }))
        .await;
    assert!(
        hidden.error().is_some(),
        "a hidden name is refused by the pattern"
    );
    let missing = harness
        .call(Who::Admin, RESTORE, json!({ "archive": "gone.tar.zst" }))
        .await;
    let id = task_of(&missing);
    assert_eq!(wait_final(&harness, id).await, TaskState::Failed);
    let failed = detail_of(&harness, id).await;
    assert_eq!(
        failed["error"]["fields"][0]["key"],
        json!(ERROR_NOT_FOUND),
        "{failed}"
    );
    assert_eq!(failed["error"]["fields"][0]["path"], json!("archive"));
    assert_eq!(
        failed["error"]["fields"][0]["params"]["archive"],
        json!("gone.tar.zst")
    );
    assert!(ERROR_NAME.starts_with("backup."));

    write_archive(
        &backups,
        "foreign.tar.zst",
        &foreign_manifest(keel_system::VERSION, "dns"),
    );
    let reply = harness
        .call(Who::Admin, RESTORE, json!({ "archive": "foreign.tar.zst" }))
        .await;
    let id = task_of(&reply);
    assert_eq!(wait_final(&harness, id).await, TaskState::Failed);
    let failed = detail_of(&harness, id).await;
    assert_eq!(
        failed["error"]["key"],
        json!("keel.error.backup_bundle"),
        "{failed}"
    );
    assert_eq!(failed["error"]["params"]["id"], json!("dns"));

    write_archive(
        &backups,
        "future.tar.zst",
        &foreign_manifest("9.0.0", "demo"),
    );
    let reply = harness
        .call(Who::Admin, RESTORE, json!({ "archive": "future.tar.zst" }))
        .await;
    let id = task_of(&reply);
    assert_eq!(wait_final(&harness, id).await, TaskState::Failed);
    let failed = detail_of(&harness, id).await;
    assert_eq!(
        failed["error"]["key"],
        json!("keel.error.backup_incompatible"),
        "{failed}"
    );

    std::fs::write(backups.join("broken.tar.zst"), "not an archive").expect("a broken file");
    let reply = harness
        .call(Who::Admin, RESTORE, json!({ "archive": "broken.tar.zst" }))
        .await;
    let id = task_of(&reply);
    assert_eq!(wait_final(&harness, id).await, TaskState::Failed);
    let failed = detail_of(&harness, id).await;
    assert_eq!(
        failed["error"]["key"],
        json!("keel.error.backup_archive"),
        "{failed}"
    );

    let menu: Vec<&str> = harness
        .registry()
        .menu()
        .iter()
        .map(|node| node.path())
        .collect();
    assert!(menu.contains(&MENU_PATH), "{menu:?}");
    harness.finish().await;
}
