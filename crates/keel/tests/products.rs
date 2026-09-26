use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use keel::sdk::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::sdk::{Registrar, migrator};
use keel::types::{Envelope, ErrorCode};
use keel::{Config, ExitCode, Running, Runtime};
use keel_kernel::backup::DUMP_MANIFEST;
use keel_kernel::dispatcher::CallOptions;
use keel_kernel::restart::EXIT_STATUS;
use keel_kernel::socket::{Client, OWNER_LOGIN};
use keel_testkit::TestDatabase;
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);
const LEDGER: &str = "SELECT id::text || ':' || timestamp::text || ':' || name FROM keel_migration_probe ORDER BY id";
const PROBE_TABLES: &str = "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_name LIKE 'probe_%' ORDER BY table_name";
const PROBE_STATE: &str = "SELECT state FROM keel_module WHERE id = 'probe'";
const RESTART_TASKS: &str =
    "SELECT function || ':' || class || ':' || state FROM task_job ORDER BY created_at";

#[derive(Deserialize, Serialize, JsonSchema)]
struct Title {
    title: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Saved {
    id: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Count {
    count: u64,
}

static PROBE_MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/probe/migrations").table("keel_migration_probe");

static PROBE_MODULE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.2.0",
    sdk: keel::sdk::VERSION,
    requires: &[],
    migrations: Some(&PROBE_MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Title, Saved>("probe.item.create")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Title>| async move {
                let id = Id::generate().to_string();
                call.tx()?
                    .execute_unprepared(&format!(
                        "INSERT INTO probe_item (id, account_id, title) VALUES ('{id}', '{}', '{}')",
                        call.principal().account_id,
                        call.params().title.replace('\'', "''")
                    ))
                    .await?;
                Ok(Saved { id })
            }),
    )
    .function(
        function::<Nothing, Count>("probe.item.count")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let row = call
                    .db()
                    .query_one(Statement::from_string(
                        DbBackend::Postgres,
                        "SELECT count(*) FROM probe_item",
                    ))
                    .await?
                    .ok_or_else(|| KeelError::internal("probe.error.count"))?;
                let count: i64 = row.try_get_by_index(0)?;
                Ok(Count {
                    count: u64::try_from(count).unwrap_or_default(),
                })
            }),
    );
    Ok(())
}

static PROBE: ProductManifest = ProductManifest {
    id: "probe",
    name: "product.probe.brand",
    version: "0.2.0",
    api: 1,
    brand: Brand {
        logo: "product.probe.logo",
        hue: 30,
        chroma: 0.1,
    },
    modules: &[&PROBE_MODULE],
    bridges: &[],
    menu: &[MenuGroup {
        id: "probe",
        label: "menu.probe",
        icon: "target",
        order: 10,
    }],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[Edition {
        id: "lite",
        features: &[],
    }],
    i18n: &[(
        "en",
        r#"{ "product": { "probe": { "brand": "Probe" } }, "menu": { "probe": "Probe" } }"#,
    )],
    assets: &[],
};

struct Site {
    database: TestDatabase,
    dir: TempDir,
    config: Config,
    config_path: PathBuf,
}

async fn site() -> Site {
    let database = TestDatabase::create().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let mut config = database.config();
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    let config_path = dir.path().join("config.yaml");
    std::fs::write(
        &config_path,
        format!(
            "listen: {}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: warn, format: text }}\n",
            config.listen,
            config.socket.display(),
            config.data_dir.display(),
            config.database.expose(),
        ),
    )
    .expect("the config file is written");
    config.file = config_path.clone();
    Site {
        database,
        dir,
        config,
        config_path,
    }
}

impl Site {
    fn runtime(&self) -> Runtime {
        Runtime::new()
            .module(&keel_system::tasks::MODULE)
            .product(&PROBE)
    }

    fn args<'a>(&'a self, rest: &[&'a str]) -> Vec<String> {
        let mut args = vec![
            "keel".to_owned(),
            "--config".to_owned(),
            self.config_path.display().to_string(),
        ];
        args.extend(rest.iter().map(|arg| (*arg).to_owned()));
        args
    }

    async fn cli(&self, runtime: Runtime, rest: &[&str]) -> ExitCode {
        let args = self.args(rest);
        tokio::task::spawn_blocking(move || runtime.run_from(args))
            .await
            .expect("the command finishes")
    }

    async fn ledger(&self) -> Vec<String> {
        self.database.rows(LEDGER).await
    }

    fn dumps(&self) -> Vec<PathBuf> {
        let backups = self.config.data_dir.join("backups");
        let Ok(entries) = std::fs::read_dir(&backups) else {
            return Vec::new();
        };
        let mut dumps: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(&format!("pre-restart-{}-", keel::VERSION))
                    })
            })
            .collect();
        dumps.sort();
        dumps
    }

    fn dump_manifest(&self, dump: &std::path::Path) -> Value {
        let text = std::fs::read_to_string(dump.join(DUMP_MANIFEST)).expect("the dump manifest");
        serde_json::from_str(&text).expect("the manifest is JSON")
    }

    async fn finish(self) {
        drop(self.dir);
        self.database.finish().await;
    }
}

async fn wait_for_socket(socket: &std::path::Path) -> Client {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match Client::connect(socket).await {
            Ok(client) => return client,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(POLL).await;
            }
            Err(error) => panic!("the socket never came up: {error}"),
        }
    }
}

async fn call(running: &Running, func: &str, params: Value) -> Envelope<Value> {
    let mut client = Client::connect(running.socket())
        .await
        .expect("the socket accepts the owner");
    client
        .call(func, params, CallOptions::default())
        .await
        .expect("the socket answers")
}

fn data(envelope: Envelope<Value>) -> Value {
    match envelope {
        Envelope::Ok { data, .. } => data,
        Envelope::Err { error } => panic!("the call failed: {error:?}"),
    }
}

async fn restart_task_ends(running: &Running, done_before: usize) -> Value {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let page = data(
            call(
                running,
                "task.list",
                json!({ "filters": [{ "key": "function", "op": "eq", "value": keel_kernel::keel::RESTART }] }),
            )
            .await,
        );
        let rows = page["data"].as_array().cloned().unwrap_or_default();
        let done: Vec<&Value> = rows
            .iter()
            .filter(|row| row["state"] == json!("done"))
            .collect();
        if done.len() > done_before {
            return done[0].clone();
        }
        if let Some(row) = rows.iter().find(|row| {
            matches!(
                row["state"].as_str(),
                Some("failed" | "cancelled" | "interrupted")
            )
        }) {
            panic!("the restart task ended without a restart: {row}");
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the restart task did not end: {rows:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn disable_leaves_the_tables_and_enable_reruns_no_applied_migration() {
    let site = site().await;
    let runtime = site.runtime();
    runtime
        .enable_product(&site.config, "probe")
        .await
        .expect("the offline enable before the first start");
    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts");
    assert_eq!(running.products(), ["probe"]);
    for title in ["alpha", "beta"] {
        data(call(&running, "probe.item.create", json!({ "title": title })).await);
    }
    let ledger_before = site.ledger().await;
    assert_eq!(ledger_before.len(), 1, "{ledger_before:?}");
    assert_eq!(site.database.rows(PROBE_TABLES).await, ["probe_item"]);
    assert_eq!(site.database.rows(PROBE_STATE).await, ["active"]);

    let disabled = data(call(&running, "keel.product.disable", json!({ "id": "probe" })).await);
    assert_eq!(
        disabled,
        json!({ "id": "probe", "state": "disabled", "changed": true, "restart": true })
    );
    let task = restart_task_ends(&running, 0).await;
    assert_eq!(task["function"], json!(keel_kernel::keel::RESTART));
    assert_eq!(task["class"], json!("restart"));
    assert_eq!(task["progress"], json!("3/3"));
    let dumps = site.dumps();
    assert_eq!(dumps.len(), 1, "{dumps:?}");
    assert_eq!(
        site.dump_manifest(&dumps[0])["reason"],
        json!("product disable probe")
    );
    running
        .stop()
        .await
        .expect("the runtime stops for the restart");

    let bare = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts again");
    assert_eq!(bare.products(), &[] as &[String]);
    assert!(!bare.modules().contains(&"probe"), "{:?}", bare.modules());
    assert_eq!(site.database.rows(PROBE_TABLES).await, ["probe_item"]);
    assert_eq!(site.database.rows(PROBE_STATE).await, ["orphaned"]);
    assert_eq!(site.ledger().await, ledger_before);
    assert_eq!(
        site.database
            .rows("SELECT title FROM probe_item ORDER BY title")
            .await,
        ["alpha", "beta"]
    );
    let refused = call(&bare, "probe.item.count", json!({})).await;
    let Envelope::Err { error } = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(error.code, ErrorCode::NotFound);
    let again = data(call(&bare, "keel.product.disable", json!({ "id": "probe" })).await);
    assert_eq!(again["changed"], json!(false));
    assert_eq!(again["restart"], json!(false));

    let enabled = data(call(&bare, "keel.product.enable", json!({ "id": "probe" })).await);
    assert_eq!(
        enabled,
        json!({ "id": "probe", "state": "enabled", "changed": true, "restart": true })
    );
    restart_task_ends(&bare, 1).await;
    assert!(
        site.dumps()
            .iter()
            .any(|dump| site.dump_manifest(dump)["reason"] == json!("product enable probe")),
        "{:?}",
        site.dumps()
    );
    bare.stop()
        .await
        .expect("the runtime stops for the restart");

    let restored = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts a third time");
    assert_eq!(restored.products(), ["probe"]);
    assert!(
        restored.modules().contains(&"probe"),
        "{:?}",
        restored.modules()
    );
    assert_eq!(site.ledger().await, ledger_before);
    assert_eq!(site.database.rows(PROBE_STATE).await, ["active"]);
    assert_eq!(
        data(call(&restored, "probe.item.count", json!({})).await),
        json!({ "count": 2 })
    );
    let tasks = site.database.rows(RESTART_TASKS).await;
    assert_eq!(
        tasks,
        ["keel.restart:restart:done", "keel.restart:restart:done"]
    );
    restored.stop().await.expect("the runtime stops");
    site.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enable_on_a_running_installation_requests_a_restart_after_the_dump() {
    let site = site().await;
    let runtime = site.runtime();
    let serving = {
        let runtime = runtime.clone();
        let args = site.args(&["serve"]);
        tokio::task::spawn_blocking(move || runtime.run_from(args))
    };
    let mut client = wait_for_socket(&site.config.socket).await;
    let listed = client
        .call("keel.product.list", json!({}), CallOptions::default())
        .await
        .expect("the socket answers");
    assert_eq!(data(listed)["data"][0]["state"], json!("disabled"));
    let missing = client
        .call("probe.item.count", json!({}), CallOptions::default())
        .await
        .expect("the socket answers");
    assert!(
        matches!(&missing, Envelope::Err { error } if error.code == ErrorCode::NotFound),
        "{missing:?}"
    );
    drop(client);

    assert_eq!(
        site.cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let exit = tokio::time::timeout(WAIT, serving)
        .await
        .expect("the service exits for the restart")
        .expect("the serve thread finishes");
    assert_eq!(exit, ExitCode::from(EXIT_STATUS));
    assert!(!site.config.socket.exists());

    let dumps = site.dumps();
    assert_eq!(dumps.len(), 1, "{dumps:?}");
    let manifest = site.dump_manifest(&dumps[0]);
    assert_eq!(manifest["reason"], json!("product enable probe"));
    assert_eq!(manifest["keel"], json!(keel::VERSION));
    assert_eq!(manifest["format"], json!("copy-text"));
    assert_eq!(manifest["schema"], json!("keel"));
    let tables: Vec<String> = manifest["tables"]
        .as_array()
        .expect("the tables")
        .iter()
        .filter_map(|table| table.as_str().map(str::to_owned))
        .collect();
    assert!(tables.contains(&"keel_kv".to_owned()), "{tables:?}");
    assert!(tables.contains(&"task_job".to_owned()), "{tables:?}");
    assert!(!tables.contains(&"probe_item".to_owned()), "{tables:?}");
    for table in &tables {
        assert!(dumps[0].join(format!("{table}.copy")).is_file(), "{table}");
    }
    let stored: Value = serde_json::from_str(
        &site
            .database
            .rows("SELECT value::text FROM keel_kv WHERE key = 'products'")
            .await[0],
    )
    .expect("the enabled set");
    assert_eq!(stored["probe"]["by"], json!(OWNER_LOGIN));
    assert_eq!(
        site.database.rows(RESTART_TASKS).await,
        ["keel.restart:restart:done"]
    );
    assert!(site.database.rows(PROBE_TABLES).await.is_empty());

    let running = runtime
        .start(&site.config)
        .await
        .expect("the next start loads the product");
    assert_eq!(running.products(), ["probe"]);
    assert!(
        running.modules().contains(&"probe"),
        "{:?}",
        running.modules()
    );
    assert_eq!(site.ledger().await.len(), 1);
    assert_eq!(
        data(call(&running, "probe.item.count", json!({})).await),
        json!({ "count": 0 })
    );
    running.stop().await.expect("the runtime stops");
    site.finish().await;
}
