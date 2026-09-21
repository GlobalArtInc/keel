use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel_sdk::db::Db;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::serde_json;
use keel_sdk::types::{ScreenResult, TaskState};
use keel_system::tasks::api::Drain;
use keel_system::update::{
    APPLY, BACKUPS_DIRECTORY, CHANNEL_KEY, CHECK, DUMP_EXTENSION, DUMP_MANIFEST, ERROR_CHANNEL,
    ERROR_CONTRACT, ERROR_CONTRACT_UNKNOWN, ERROR_MIN_UPGRADE_FROM, ERROR_NO_PREVIOUS,
    ERROR_UP_TO_DATE, MODULE, REQUEST_FILE, ROLLBACK, SECTION,
};
use keel_testkit::{Harness, Reply, Who, tokio};
use tokio::net::TcpListener;

const RUNNING: &str = keel_system::VERSION;
const WAIT: Duration = Duration::from_secs(30);
const SLOW: &str = "probe.slow";
const QUICK: &str = "probe.quick";
const PROBE_CLASS: &str = "probe";

static CHANNELS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Ended {
    ended: bool,
}

async fn database_name(db: &Db) -> Result<String> {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database() AS name",
        ))
        .await?
        .ok_or_else(|| KeelError::internal("probe.error.no_database"))?;
    Ok(row.try_get::<String>("", "name")?)
}

fn choose_channel(harness: &Harness, channel: &str) {
    CHANNELS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(harness.database().name().to_owned(), channel.to_owned());
}

struct Chosen;

#[async_trait]
impl SettingsSource for Chosen {
    async fn resolve(&self, key: &str, _: Id, db: &Db) -> Result<Option<Value>> {
        if key != CHANNEL_KEY {
            return Ok(None);
        }
        let name = database_name(db).await?;
        let channel = CHANNELS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|map| map.get(&name).cloned())
            .unwrap_or_else(|| "stable".to_owned());
        Ok(Some(json!(channel)))
    }
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["task"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn SettingsSource>(Arc::new(Chosen))
        .i18n(
            "en",
            r#"{ "probe": { "slow": { "title": "Slow" }, "quick": { "title": "Quick" } } }"#,
        )
        .i18n(
            "ru",
            r#"{ "probe": { "slow": { "title": "Медленная" }, "quick": { "title": "Быстрая" } } }"#,
        )
        .function(
            function::<Nothing, Ended>(SLOW)
                .kind(Kind::Action)
                .level(Level::Admin)
                .unowned()
                .class(PROBE_CLASS)
                .ui(ActionScreen::new("probe.slow.title").result(ScreenResult::Task))
                .handler(|call: Call<Nothing>| async move {
                    let task = call
                        .task()
                        .ok_or_else(|| KeelError::internal("probe.error.inline"))?;
                    for step in 0..1200u64 {
                        if task.is_cancelled() {
                            return Err(task.cancelled());
                        }
                        task.progress(step, None, None).await?;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Ok(Ended { ended: true })
                }),
        )
        .function(
            function::<Nothing, Ended>(QUICK)
                .kind(Kind::Action)
                .level(Level::Admin)
                .unowned()
                .class(PROBE_CLASS)
                .ui(ActionScreen::new("probe.quick.title").result(ScreenResult::Task))
                .handler(|_: Call<Nothing>| async move { Ok(Ended { ended: true }) }),
        );
    Ok(())
}

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

struct Channel {
    port: u16,
    files: Files,
}

async fn serve_file(
    UrlPath(path): UrlPath<String>,
    State(files): State<Files>,
) -> impl IntoResponse {
    let found = files
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&path)
        .cloned();
    match found {
        Some(bytes) => (StatusCode::OK, bytes).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn signed(version: &str, min_upgrade_from: &str, contract: u32, notes: &str) -> Vec<u8> {
    let manifest = json!({
        "version": version,
        "sha256": "00",
        "min_upgrade_from": min_upgrade_from,
        "contract": contract,
        "notes": notes,
    });
    json!({
        "payload": STANDARD.encode(manifest.to_string()),
        "signature": STANDARD.encode([7u8; 64]),
    })
    .to_string()
    .into_bytes()
}

impl Channel {
    async fn serve() -> Self {
        let files: Files = Arc::new(Mutex::new(HashMap::new()));
        let router = Router::new()
            .route("/channel/{*path}", get(serve_file))
            .with_state(files.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let port = listener.local_addr().expect("an address").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { port, files }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/channel/", self.port)
    }

    fn head(&self, channel: &str, version: &str, min_upgrade_from: &str, contract: u32) {
        self.publish(
            &format!("{channel}/manifest.json"),
            signed(
                version,
                min_upgrade_from,
                contract,
                &format!("Release {version} on {channel}"),
            ),
        );
    }

    fn version(&self, version: &str, min_upgrade_from: &str, contract: u32) {
        self.publish(
            &format!("versions/{version}/manifest.json"),
            signed(
                version,
                min_upgrade_from,
                contract,
                &format!("Release {version}"),
            ),
        );
    }

    fn publish(&self, path: &str, bytes: Vec<u8>) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned(), bytes);
    }

    fn remove(&self, path: &str) {
        self.files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(path);
    }
}

async fn boot(channel: &Channel) -> Harness {
    Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .section(SECTION, json!({ "channel_url": channel.url() }))
        .boot()
        .await
}

fn drain(harness: &Harness) -> Arc<Drain> {
    harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain")
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
    tokio::time::timeout(WAIT, drain(harness).wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

async fn wait_state(harness: &Harness, id: Id, state: &str) {
    let sql = format!("SELECT state FROM task_job WHERE id = '{id}'");
    for _ in 0..600 {
        if harness.database().rows(&sql).await == [state] {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job {id} did not reach {state}");
}

async fn task_error(harness: &Harness, id: Id) -> Value {
    let reply = harness
        .call(Who::Admin, "task.get", json!({ "id": id }))
        .await;
    data(&reply)["error"].clone()
}

async fn run_task(harness: &Harness, func: &str) -> (Id, TaskState) {
    let reply = harness.call(Who::Admin, func, json!({})).await;
    let id = task_of(&reply);
    let state = wait_final(harness, id).await;
    (id, state)
}

async fn tables_of(harness: &Harness) -> BTreeSet<String> {
    harness
        .database()
        .rows("SELECT table_name FROM information_schema.tables WHERE table_schema = 'keel' AND table_type = 'BASE TABLE' ORDER BY table_name")
        .await
        .into_iter()
        .collect()
}

fn dumps_of(harness: &Harness) -> Vec<std::path::PathBuf> {
    let backups = harness.data_dir().join(BACKUPS_DIRECTORY);
    let mut dumps: Vec<_> = std::fs::read_dir(&backups)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect()
        })
        .unwrap_or_default();
    dumps.sort();
    dumps
}

async fn record_run(harness: &Harness, from: &str, to: &str) {
    let sql = format!(
        "INSERT INTO update_run (id, kind, from_version, to_version, channel, requested_by, principal_id, request_id, dump_dir, tables, requested_at) VALUES ('{}', 'apply', '{from}', '{to}', 'stable', 'root', '{}', 'req-seed', '/nowhere', 0, '2020-01-01T00:00:00Z')",
        Id::generate(),
        Id::ROOT_ACCOUNT
    );
    harness
        .db()
        .execute_unprepared(&sql)
        .await
        .expect("the run is recorded");
}

#[tokio::test]
async fn check_compares_the_running_version_with_the_channel_head_and_reads_min_upgrade_from() {
    let channel = Channel::serve().await;
    channel.head("stable", "0.2.0", RUNNING, 1);
    channel.head("beta", "0.3.0-beta.1", "0.2.0", 1);
    channel.version(RUNNING, "0.0.1", 1);
    let harness = boot(&channel).await;

    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["running"], json!(RUNNING));
    assert_eq!(status["runningContract"], json!(1));
    assert_eq!(status["channel"], json!("stable"));
    assert_eq!(status["channelUrl"], json!(channel.url()));
    assert_eq!(status["available"], json!("0.2.0"));
    assert_eq!(status["notes"], json!("Release 0.2.0 on stable"));
    assert_eq!(status["minUpgradeFrom"], json!(RUNNING));
    assert_eq!(status["contract"], json!(1));
    assert_eq!(status["upToDate"], json!(false));
    assert_eq!(status["allowed"], json!(true));
    assert_eq!(status["blockedBy"], Value::Null);
    assert_eq!(status["previous"], Value::Null);

    channel.head("stable", "0.3.0", "0.2.0", 1);
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["available"], json!("0.3.0"));
    assert_eq!(status["minUpgradeFrom"], json!("0.2.0"));
    assert_eq!(status["allowed"], json!(false));
    assert_eq!(status["blockedBy"], json!("min_upgrade_from"));

    channel.head("stable", "1.0.0", "0.1.0", 2);
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["allowed"], json!(false));
    assert_eq!(status["blockedBy"], json!("contract"));
    assert_eq!(status["contract"], json!(2));

    channel.head("stable", RUNNING, "0.0.1", 1);
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["upToDate"], json!(true));
    assert_eq!(status["allowed"], json!(false));
    assert_eq!(status["blockedBy"], Value::Null);

    choose_channel(&harness, "beta");
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["channel"], json!("beta"));
    assert_eq!(status["available"], json!("0.3.0-beta.1"));
    assert_eq!(status["notes"], json!("Release 0.3.0-beta.1 on beta"));
    assert_eq!(status["blockedBy"], json!("min_upgrade_from"));
    choose_channel(&harness, "stable");

    channel.remove(&format!("versions/{RUNNING}/manifest.json"));
    channel.head("stable", "0.2.0", RUNNING, 2);
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    let status = data(&checked);
    assert_eq!(status["runningContract"], Value::Null);
    assert_eq!(status["allowed"], json!(true));

    channel.remove("stable/manifest.json");
    let unreachable = harness.call(Who::Admin, CHECK, json!({})).await;
    let error = unreachable.error().expect("the head is gone");
    assert_eq!(error.key, ERROR_CHANNEL);
    assert_eq!(
        error.params["url"],
        json!(format!("{}stable/manifest.json", channel.url()))
    );
    channel.publish("stable/manifest.json", b"not a manifest".to_vec());
    let broken = harness.call(Who::Admin, CHECK, json!({})).await;
    assert_eq!(
        broken.error().expect("the head is broken").key,
        keel_system::update::ERROR_MANIFEST
    );

    let refused = harness.call(Who::Reseller, CHECK, json!({})).await;
    assert_eq!(refused.status(), 403);
    harness.finish().await;
}

#[tokio::test]
async fn apply_drains_dumps_every_loaded_table_and_writes_the_request_with_the_principal() {
    let channel = Channel::serve().await;
    channel.head("stable", "0.2.0", RUNNING, 1);
    channel.version(RUNNING, "0.0.1", 1);
    let harness = boot(&channel).await;
    let drain = drain(&harness);

    let slow = task_of(&harness.call(Who::Admin, SLOW, json!({})).await);
    let quick = task_of(&harness.call(Who::Admin, QUICK, json!({})).await);
    wait_state(&harness, slow, "running").await;
    assert!(!drain.is_quiesced().expect("the runner started"));

    let (id, state) = run_task(&harness, APPLY).await;
    assert_eq!(
        state,
        TaskState::Done,
        "{:?}",
        task_error(&harness, id).await
    );
    assert_eq!(wait_final(&harness, slow).await, TaskState::Interrupted);
    assert_eq!(
        harness
            .database()
            .rows(&format!("SELECT state FROM task_job WHERE id = '{quick}'"))
            .await,
        ["queued"]
    );
    assert!(drain.is_quiesced().expect("the runner started"));

    let admin = harness.principal(Who::Admin).clone();
    let request = std::fs::read_to_string(harness.data_dir().join(REQUEST_FILE))
        .expect("the request is written");
    assert_eq!(
        serde_json::from_str::<Value>(&request).expect("json"),
        json!({ "version": "0.2.0", "channel": "stable", "requested_by": admin.login })
    );

    let dumps = dumps_of(&harness);
    assert_eq!(dumps.len(), 1, "{dumps:?}");
    let dump = &dumps[0];
    let name = dump.file_name().and_then(|name| name.to_str()).unwrap();
    assert!(
        name.starts_with(&format!("pre-update-{RUNNING}-")),
        "{name}"
    );
    let tables = tables_of(&harness).await;
    assert!(tables.contains("task_job"));
    assert!(tables.contains("update_run"));
    assert!(tables.contains("keel_kv"));
    let mut dumped: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(dump).expect("the dump directory") {
        let path = entry.expect("an entry").path();
        let file = path.file_name().and_then(|name| name.to_str()).unwrap();
        if file == DUMP_MANIFEST {
            continue;
        }
        let table = file
            .strip_suffix(&format!(".{DUMP_EXTENSION}"))
            .unwrap_or_else(|| panic!("{file} is not a table dump"));
        dumped.insert(table.to_owned());
    }
    assert_eq!(dumped, tables);
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(dump.join(DUMP_MANIFEST)).expect("the manifest"))
            .expect("json");
    assert_eq!(manifest["keel"], json!(RUNNING));
    assert_eq!(manifest["schema"], json!("keel"));
    assert_eq!(
        manifest["tables"].as_array().map(Vec::len),
        Some(tables.len())
    );
    let jobs = std::fs::read_to_string(dump.join(format!("task_job.{DUMP_EXTENSION}")))
        .expect("the task table is dumped");
    assert!(jobs.contains(&slow.to_string()), "{jobs}");
    assert!(jobs.contains(APPLY), "{jobs}");

    let runs = harness
        .database()
        .rows("SELECT kind || ' ' || from_version || ' ' || to_version || ' ' || channel || ' ' || requested_by || ' ' || tables::text FROM update_run")
        .await;
    assert_eq!(
        runs,
        [format!(
            "apply {RUNNING} 0.2.0 stable {} {}",
            admin.login,
            tables.len()
        )]
    );

    let detail = harness
        .call(Who::Admin, "task.get", json!({ "id": id }))
        .await;
    let detail = data(&detail);
    assert_eq!(detail["result"]["kind"], json!("apply"));
    assert_eq!(detail["result"]["from"], json!(RUNNING));
    assert_eq!(detail["result"]["to"], json!("0.2.0"));
    assert_eq!(detail["result"]["requestedBy"], json!(admin.login));
    assert_eq!(detail["result"]["tables"], json!(tables.len()));
    assert_eq!(detail["result"]["leftRunning"], json!(0));
    assert_eq!(detail["progress"], json!("3/3"));
    let log = detail["logTail"].to_string();
    assert!(log.contains("the root unit takes over"), "{log}");
    assert!(log.contains("the task queue is drained"), "{log}");

    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    assert_eq!(data(&checked)["previous"], Value::Null);
    harness.finish().await;
}

#[tokio::test]
async fn apply_refuses_a_version_below_min_upgrade_from_and_across_a_contract() {
    let channel = Channel::serve().await;
    channel.version(RUNNING, "0.0.1", 1);
    let harness = boot(&channel).await;
    let drain = drain(&harness);

    channel.head("stable", "0.3.0", "0.2.0", 1);
    let (id, state) = run_task(&harness, APPLY).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_MIN_UPGRADE_FROM));
    assert_eq!(error["code"], json!("conflict"));
    assert_eq!(error["params"]["running"], json!(RUNNING));
    assert_eq!(error["params"]["target"], json!("0.3.0"));
    assert_eq!(error["params"]["minUpgradeFrom"], json!("0.2.0"));

    channel.head("stable", "1.0.0", RUNNING, 2);
    let (id, state) = run_task(&harness, APPLY).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_CONTRACT));
    assert_eq!(error["params"]["runningContract"], json!(1));
    assert_eq!(error["params"]["targetContract"], json!(2));
    assert_eq!(error["params"]["target"], json!("1.0.0"));

    channel.head("stable", RUNNING, "0.0.1", 1);
    let (id, state) = run_task(&harness, APPLY).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_UP_TO_DATE));

    channel.remove("stable/manifest.json");
    let (id, state) = run_task(&harness, APPLY).await;
    assert_eq!(state, TaskState::Failed);
    assert_eq!(task_error(&harness, id).await["key"], json!(ERROR_CHANNEL));

    assert!(!harness.data_dir().join(REQUEST_FILE).exists());
    assert!(dumps_of(&harness).is_empty());
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM update_run")
            .await,
        ["0"]
    );
    assert!(!drain.is_quiesced().expect("the runner started"));
    let (_, state) = run_task(&harness, QUICK).await;
    assert_eq!(state, TaskState::Done);
    harness.finish().await;
}

#[tokio::test]
async fn rollback_writes_a_rollback_request() {
    let channel = Channel::serve().await;
    channel.head("stable", RUNNING, "0.0.9", 1);
    channel.version(RUNNING, "0.0.9", 1);
    channel.version("0.0.9", "0.0.1", 1);
    let harness = boot(&channel).await;
    let drain = drain(&harness);

    let (id, state) = run_task(&harness, ROLLBACK).await;
    assert_eq!(state, TaskState::Failed);
    assert_eq!(
        task_error(&harness, id).await["key"],
        json!(ERROR_NO_PREVIOUS)
    );

    record_run(&harness, "0.0.9", RUNNING).await;
    let checked = harness.call(Who::Admin, CHECK, json!({})).await;
    assert_eq!(data(&checked)["previous"], json!("0.0.9"));
    assert_eq!(data(&checked)["upToDate"], json!(true));

    channel.version("0.0.9", "0.0.1", 0);
    let (id, state) = run_task(&harness, ROLLBACK).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_CONTRACT));
    assert_eq!(error["params"]["running"], json!(RUNNING));
    assert_eq!(error["params"]["runningContract"], json!(1));
    assert_eq!(error["params"]["target"], json!("0.0.9"));
    assert_eq!(error["params"]["targetContract"], json!(0));

    channel.remove("versions/0.0.9/manifest.json");
    let (id, state) = run_task(&harness, ROLLBACK).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_CONTRACT_UNKNOWN));
    assert_eq!(error["params"]["version"], json!("0.0.9"));

    channel.version("0.0.9", "0.0.1", 1);
    channel.head("stable", RUNNING, RUNNING, 1);
    let (id, state) = run_task(&harness, ROLLBACK).await;
    assert_eq!(state, TaskState::Failed);
    let error = task_error(&harness, id).await;
    assert_eq!(error["key"], json!(ERROR_MIN_UPGRADE_FROM));
    assert_eq!(error["params"]["target"], json!("0.0.9"));
    assert!(!harness.data_dir().join(REQUEST_FILE).exists());
    assert!(!drain.is_quiesced().expect("the runner started"));

    channel.head("stable", RUNNING, "0.0.9", 1);
    let (id, state) = run_task(&harness, ROLLBACK).await;
    assert_eq!(
        state,
        TaskState::Done,
        "{:?}",
        task_error(&harness, id).await
    );
    let admin = harness.principal(Who::Admin).clone();
    let request = std::fs::read_to_string(harness.data_dir().join(REQUEST_FILE))
        .expect("the request is written");
    assert_eq!(
        serde_json::from_str::<Value>(&request).expect("json"),
        json!({ "version": "0.0.9", "channel": "stable", "requested_by": admin.login })
    );
    assert_eq!(dumps_of(&harness).len(), 1);
    assert!(drain.is_quiesced().expect("the runner started"));
    let runs = harness
        .database()
        .rows("SELECT kind || ' ' || from_version || ' ' || to_version || ' ' || requested_by FROM update_run ORDER BY requested_at, id")
        .await;
    assert_eq!(
        runs,
        [
            format!("apply 0.0.9 {RUNNING} root"),
            format!("rollback {RUNNING} 0.0.9 {}", admin.login),
        ]
    );
    let detail = harness
        .call(Who::Admin, "task.get", json!({ "id": id }))
        .await;
    assert_eq!(data(&detail)["result"]["kind"], json!("rollback"));
    assert_eq!(data(&detail)["result"]["to"], json!("0.0.9"));
    harness.finish().await;
}

#[cfg(feature = "notify")]
#[tokio::test]
async fn the_daily_check_raises_update_available_and_the_running_version_clears_it() {
    use keel_system::cron::api::{Outcome, Scheduler};
    use keel_system::update::{POLL_JOB, PROBLEM_KEY};

    let channel = Channel::serve().await;
    channel.head("stable", "0.2.0", RUNNING, 1);
    channel.version(RUNNING, "0.0.1", 1);
    let harness = Harness::new()
        .module(&keel_system::tasks::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .section(SECTION, json!({ "channel_url": channel.url() }))
        .boot()
        .await;
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    for _ in 0..200 {
        if scheduler.is_leader() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(scheduler.jobs().iter().any(|job| job == POLL_JOB));

    async fn run(harness: &Harness, scheduler: &Scheduler) -> Outcome {
        let started = harness
            .call(
                Who::Admin,
                keel_system::cron::JOB_RUN,
                json!({ "id": POLL_JOB }),
            )
            .await;
        let run: Id = data(&started)["run"]
            .as_str()
            .expect("a run id")
            .parse()
            .expect("a ulid");
        tokio::time::timeout(WAIT, scheduler.wait(run))
            .await
            .expect("the run ends in time")
            .expect("the outcome is read")
    }
    async fn open(harness: &Harness) -> Vec<String> {
        harness
            .database()
            .rows("SELECT key || ' ' || subject || ' ' || severity || ' ' || source || ' ' || count::text FROM notify_problem WHERE cleared_at IS NULL ORDER BY key, subject")
            .await
    }

    assert_eq!(run(&harness, &scheduler).await, Outcome::Ok);
    assert_eq!(
        open(&harness).await,
        [format!("{PROBLEM_KEY} stable warning {POLL_JOB} 1")]
    );
    let listed = harness
        .call(Who::Admin, keel_system::notify::PROBLEM_LIST, json!({}))
        .await;
    let problems = data(&listed)["data"].clone();
    assert_eq!(problems.as_array().map(Vec::len), Some(1), "{problems}");
    assert_eq!(problems[0]["key"], json!(PROBLEM_KEY));
    assert_eq!(problems[0]["subject"], json!("stable"));
    assert_eq!(problems[0]["open"], json!(true));
    assert_eq!(problems[0]["params"]["available"], json!("0.2.0"));
    assert_eq!(problems[0]["params"]["running"], json!(RUNNING));
    assert_eq!(problems[0]["params"]["allowed"], json!(true));

    assert_eq!(run(&harness, &scheduler).await, Outcome::Ok);
    assert_eq!(
        open(&harness).await,
        [format!("{PROBLEM_KEY} stable warning {POLL_JOB} 2")]
    );

    channel.head("stable", RUNNING, "0.0.1", 1);
    assert_eq!(run(&harness, &scheduler).await, Outcome::Ok);
    assert!(open(&harness).await.is_empty());
    let listed = harness
        .call(Who::Admin, keel_system::notify::PROBLEM_LIST, json!({}))
        .await;
    assert_eq!(data(&listed)["data"][0]["key"], json!(PROBLEM_KEY));
    assert_eq!(data(&listed)["data"][0]["open"], json!(false));

    channel.remove("stable/manifest.json");
    assert_eq!(run(&harness, &scheduler).await, Outcome::Failed);
    harness.finish().await;
}
