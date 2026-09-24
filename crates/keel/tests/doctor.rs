use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::time::Duration;

use keel::sdk::prelude::*;
use keel::sdk::{Registrar, migrator};
use keel::types::Envelope;
use keel::{Config, ExitCode, Running, Runtime};
use keel_kernel::doctor::{self, Database, Doctor, Layout, Report, Status};
use keel_kernel::loader::{Composition, Sections};
use keel_kernel::socket::Client;
use keel_kernel::{Storage, logging, support};
use keel_testkit::TestDatabase;
use tempfile::TempDir;

const CHANNEL_PASSWORD: &str = "relay-pass-7f3a9c";
const UNIT_FILES: [(&str, &str); 4] = [
    (
        "keel.service",
        include_str!("../../../deploy/systemd/keel.service"),
    ),
    (
        "keel-privileged.service",
        include_str!("../../../deploy/systemd/keel-privileged.service"),
    ),
    (
        "keel-update.service",
        include_str!("../../../deploy/systemd/keel-update.service"),
    ),
    (
        "keel-update.path",
        include_str!("../../../deploy/systemd/keel-update.path"),
    ),
];

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Default, Deserialize, JsonSchema)]
struct ProbeSection {
    endpoint: String,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    dsn: String,
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
    r.contribute(ConfigSection::new::<ProbeSection>("probe"));
    r.function(
        function::<Nothing, Nothing>("probe.ping")
            .kind(Kind::Get)
            .level(Level::Admin)
            .unowned()
            .handler(|_: Call<Nothing>| async move { Ok(Nothing {}) }),
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

const MODULES: [&ModuleDef; 5] = [
    &keel_system::auth::MODULE,
    &keel_system::tasks::MODULE,
    &keel_system::cron::MODULE,
    &keel_system::notify::MODULE,
    &keel_system::audit::MODULE,
];

fn free_port() -> SocketAddr {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a free port");
    let port = listener.local_addr().expect("the bound address").port();
    drop(listener);
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

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
    config.listen = free_port();
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    let etc = dir.path().join("etc");
    fs::create_dir_all(&etc).expect("the config directory");
    fs::set_permissions(&etc, fs::Permissions::from_mode(0o750)).expect("chmod");
    let config_path = etc.join("config.yaml");
    fs::write(
        &config_path,
        format!(
            "listen: {}\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: info, format: text }}\nrelay: {{ host: relay.example, password: never-in-a-bundle, api_key: sk-never-in-a-bundle }}\nprobe: {{ endpoint: probe.example, dsn: dsn-never-in-a-bundle }}\n",
            config.listen,
            config.socket.display(),
            config.data_dir.display(),
            config.database.expose(),
        ),
    )
    .expect("the config file is written");
    fs::set_permissions(&config_path, fs::Permissions::from_mode(0o640)).expect("chmod");
    fs::write(etc.join("agent.yaml"), "policy:\n  services: [nginx]\n").expect("the policy");
    config.file = config_path.clone();
    config.sections.insert(
        "relay".to_owned(),
        json!({
            "host": "relay.example",
            "password": "never-in-a-bundle",
            "api_key": "sk-never-in-a-bundle"
        }),
    );
    config.sections.insert(
        "probe".to_owned(),
        json!({ "endpoint": "probe.example", "dsn": "dsn-never-in-a-bundle" }),
    );
    Site {
        database,
        dir,
        config,
        config_path,
    }
}

impl Site {
    fn runtime(&self) -> Runtime {
        MODULES
            .iter()
            .fold(Runtime::new(), |runtime, module| runtime.module(module))
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

    fn layout(&self) -> Layout {
        let root = self.dir.path();
        let units = root.join("units");
        fs::create_dir_all(&units).expect("the unit directory");
        for (name, text) in UNIT_FILES {
            fs::write(units.join(name), text).expect("a unit file");
        }
        let opt = root.join("opt");
        let version_dir = opt.join("versions").join(keel::VERSION);
        fs::create_dir_all(&version_dir).expect("the version directory");
        fs::create_dir_all(opt.join("bin")).expect("the bin directory");
        let binary = version_dir.join("keel");
        fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("the binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod");
        symlink(&binary, opt.join("bin").join("current")).expect("the current link");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("the stub directory");
        let systemctl = bin.join("systemctl");
        fs::write(
            &systemctl,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\ncase \"$1\" in is-enabled) echo enabled ;; is-active) echo active ;; esac\n",
                bin.join("systemctl.log").display()
            ),
        )
        .expect("the stub");
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).expect("chmod");
        Layout {
            opt,
            unit_dir: units,
            systemctl,
            agent_socket: root.join("agent.sock"),
        }
    }

    fn layout_args(layout: &Layout) -> Vec<String> {
        vec![
            "--opt".to_owned(),
            layout.opt.display().to_string(),
            "--unit-dir".to_owned(),
            layout.unit_dir.display().to_string(),
            "--systemctl".to_owned(),
            layout.systemctl.display().to_string(),
            "--agent-socket".to_owned(),
            layout.agent_socket.display().to_string(),
        ]
    }

    async fn report(&self, enabled: &[&str], layout: &Layout) -> Report {
        let mut composition = Composition::new().version(keel::VERSION);
        for module in MODULES {
            composition = composition.module(module);
        }
        let graph = composition
            .product(&PROBE)
            .enabled(enabled.iter().map(|id| (*id).to_owned()))
            .sections(Sections::new(
                &self.config.file,
                self.config.sections.clone(),
            ))
            .data_dir(self.config.data_dir.clone())
            .resolve()
            .expect("the composition resolves");
        let doctor = Doctor::new(&graph, &self.config.tls, layout.clone());
        let storage = Storage::connect(&self.config)
            .await
            .expect("the site connects");
        let report = doctor.run(&Database::Connected(storage.db())).await;
        storage.close().await.expect("the pool closes");
        report
    }

    async fn finish(self) {
        drop(self.dir);
        self.database.finish().await;
    }
}

async fn owner_call(running: &Running, func: &str, params: Value, run_as: Option<&str>) -> Value {
    let mut client = Client::connect(running.socket())
        .await
        .expect("the socket accepts the owner");
    let envelope = client
        .call_as(func, params, run_as, Default::default())
        .await
        .expect("the socket answers");
    match envelope {
        Envelope::Ok { data, .. } => data,
        Envelope::Err { error } => panic!("{func} failed: {error:?}"),
    }
}

fn init_logging(config: &Config) {
    let mut log = config.log.clone();
    log.level = "info".to_owned();
    let _ = logging::init(&log);
}

fn statuses(report: &Report) -> Vec<(String, Status)> {
    report
        .checks
        .iter()
        .map(|check| (check.id.clone(), check.status))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_is_green_on_a_fresh_site_and_names_a_missing_unit_a_bad_mode_and_an_orphaned_module()
 {
    let site = site().await;
    let runtime = site.runtime();
    assert_eq!(
        site.cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime.start(&site.config).await.expect("the site starts");
    let layout = site.layout();
    let layout_args = Site::layout_args(&layout);
    let mut args: Vec<&str> = vec!["doctor"];
    args.extend(layout_args.iter().map(String::as_str));
    assert_eq!(
        site.cli(runtime.clone(), &args).await,
        ExitCode::SUCCESS,
        "the doctor is green on a fresh site"
    );
    let report = site.report(&["probe"], &layout).await;
    assert_eq!(
        report
            .checks
            .iter()
            .map(|check| check.id.as_str())
            .collect::<Vec<_>>(),
        doctor::CHECKS
    );
    for check in &report.checks {
        assert_eq!(check.status, Status::Ok, "{}", check.line());
    }
    assert!(!report.failed());
    let rendered = report.render();
    assert!(rendered.contains("14 ok, 0 warn, 0 fail"), "{rendered}");
    assert!(
        rendered.contains("probe enabled and carried by the binary"),
        "{rendered}"
    );
    assert!(rendered.contains("not in use"), "{rendered}");
    assert!(rendered.contains("PostgreSQL"), "{rendered}");
    let calls = fs::read_to_string(layout.systemctl.with_file_name("systemctl.log")).unwrap();
    assert!(
        calls.contains("is-enabled keel-update.path"),
        "the stub service manager was asked: {calls}"
    );
    let over_socket = owner_call(&running, "keel.doctor.run", json!({}), None).await;
    let rows = over_socket["data"].as_array().expect("rows");
    assert_eq!(rows.len(), doctor::CHECKS.len());
    assert_eq!(rows[0]["check"], json!("data_dir"));
    assert_eq!(rows[0]["status"], json!("ok"));
    let only_fail = owner_call(
        &running,
        "keel.doctor.run",
        json!({ "filters": [{ "key": "status", "op": "eq", "value": "fail" }] }),
        None,
    )
    .await;
    let failing: Vec<&str> = only_fail["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["check"].as_str().unwrap_or_default())
        .collect();
    assert!(
        failing
            .iter()
            .all(|check| ["install_dir", "units"].contains(check)),
        "with the design's layout only the units and the install directory fail here: {failing:?}"
    );
    running.stop().await.expect("the site stops");

    assert_eq!(
        site.cli(runtime.clone(), &["product", "disable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&site.config)
        .await
        .expect("the site starts without the probe");
    fs::remove_file(layout.unit_dir.join("keel-update.path")).unwrap();
    let secret_key = site.config.data_dir.join("secret.key");
    fs::set_permissions(&secret_key, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        site.cli(runtime.clone(), &args).await,
        ExitCode::FAILURE,
        "a missing unit and an open key fail the doctor"
    );
    let report = site.report(&[], &layout).await;
    let named = statuses(&report);
    assert!(
        named.contains(&("units".to_owned(), Status::Fail)),
        "{named:?}"
    );
    assert!(
        named.contains(&("secret_key".to_owned(), Status::Fail)),
        "{named:?}"
    );
    assert!(
        named.contains(&("orphans".to_owned(), Status::Warn)),
        "{named:?}"
    );
    assert!(
        named.contains(&("products".to_owned(), Status::Ok)),
        "{named:?}"
    );
    let units = report.check("units").expect("the units check");
    assert!(
        units.message.contains("keel-update.path is missing from"),
        "{}",
        units.message
    );
    assert!(
        !units.message.contains("keel.service is missing"),
        "{}",
        units.message
    );
    let key = report.check("secret_key").expect("the key check");
    assert!(
        key.message.contains("secret.key has mode 0644"),
        "{}",
        key.message
    );
    let orphans = report.check("orphans").expect("the orphans check");
    assert!(orphans.message.contains("probe"), "{}", orphans.message);
    assert!(
        orphans.message.contains("tables remain"),
        "{}",
        orphans.message
    );
    let over_socket = owner_call(
        &running,
        "keel.doctor.run",
        json!({ "filters": [{ "key": "status", "op": "ne", "value": "ok" }] }),
        None,
    )
    .await;
    let rows = over_socket["data"].as_array().expect("rows");
    let orphan_row = rows
        .iter()
        .find(|row| row["check"] == json!("orphans"))
        .expect("the screen lists the orphaned module");
    assert_eq!(orphan_row["status"], json!("warn"));
    assert!(
        orphan_row["message"]
            .as_str()
            .unwrap_or_default()
            .contains("probe")
    );
    let key_row = rows
        .iter()
        .find(|row| row["check"] == json!("secret_key"))
        .expect("the screen lists the open key");
    assert_eq!(key_row["status"], json!("fail"));
    fs::set_permissions(&secret_key, fs::Permissions::from_mode(0o600)).unwrap();
    running.stop().await.expect("the site stops");
    site.finish().await;
}

async fn wait_for_audit(site: &Site, function: &str) {
    for _ in 0..40 {
        let count = site
            .database
            .rows(&format!(
                "SELECT count(*)::text FROM audit_call WHERE function = '{function}'"
            ))
            .await;
        if count.first().is_some_and(|count| count != "0") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the audit trail never recorded {function}");
}

fn entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> &'a [u8] {
    entries
        .iter()
        .find(|(entry, _)| entry == name)
        .map(|(_, bytes)| bytes.as_slice())
        .unwrap_or_else(|| panic!("{name} is in the bundle"))
}

fn contains(bytes: &[u8], needle: &str) -> bool {
    bytes
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_support_bundle_carries_no_secret_value() {
    let site = site().await;
    init_logging(&site.config);
    let runtime = site.runtime();
    assert_eq!(
        site.cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&site.config)
        .await
        .expect("the site starts with the probe");
    running.stop().await.expect("the site stops");
    assert_eq!(
        site.cli(runtime.clone(), &["product", "disable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&site.config)
        .await
        .expect("the site starts without the probe");
    let probe: ProbeSection =
        serde_json::from_value(site.config.sections["probe"].clone()).expect("the section");
    assert_eq!(
        (probe.endpoint.as_str(), probe.dsn.as_str()),
        ("probe.example", "dsn-never-in-a-bundle")
    );
    let channel = owner_call(
        &running,
        "notify.channel.set",
        json!({
            "kind": "email",
            "name": "relay",
            "email": {
                "host": "smtp.example",
                "port": 587,
                "security": "starttls",
                "username": "keel",
                "password": CHANNEL_PASSWORD,
                "from": "keel@example.org",
                "to": "ops@example.org"
            }
        }),
        None,
    )
    .await;
    assert!(channel["id"].is_string(), "{channel}");
    let account = owner_call(
        &running,
        "keel.account.create",
        json!({ "kind": "user" }),
        None,
    )
    .await;
    owner_call(
        &running,
        "keel.principal.create",
        json!({ "login": "alice", "level": "user", "account": account["id"] }),
        None,
    )
    .await;
    let created = owner_call(
        &running,
        "auth.token.create",
        json!({ "name": "support-probe" }),
        Some("alice"),
    )
    .await;
    let token = created["token"].as_str().expect("the plaintext").to_owned();
    assert!(token.starts_with("kat_"), "{token}");
    tracing::info!(token_id = %created["id"], "the probe token was created");
    wait_for_audit(&site, "notify.channel.set").await;
    wait_for_audit(&site, "auth.token.create").await;

    let out = site.dir.path().join("bundle.tar.zst");
    let layout = site.layout();
    let layout_args = Site::layout_args(&layout);
    let mut args: Vec<&str> = vec!["support-bundle", "--out", out.to_str().expect("utf-8")];
    args.extend(layout_args.iter().map(String::as_str));
    assert_eq!(site.cli(runtime.clone(), &args).await, ExitCode::SUCCESS);
    assert_eq!(
        site.cli(runtime.clone(), &args).await,
        ExitCode::FAILURE,
        "a bundle never overwrites"
    );
    let entries = support::read_entries(&out).expect("the bundle opens");
    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "config.yaml",
        "agent.yaml",
        "logs.jsonl",
        "ledgers.json",
        "audit.jsonl",
        "problems.json",
        "readiness.json",
        "doctor.txt",
        "manifest.json",
    ] {
        assert!(names.contains(&expected), "{expected} in {names:?}");
    }
    for (name, bytes) in &entries {
        assert!(
            !contains(bytes, CHANNEL_PASSWORD),
            "{name} carries the channel password"
        );
        assert!(!contains(bytes, &token), "{name} carries the token");
        assert!(
            !contains(bytes, "never-in-a-bundle"),
            "{name} carries the configuration secret"
        );
    }
    let config_text = String::from_utf8_lossy(entry(&entries, "config.yaml")).into_owned();
    assert!(config_text.contains("host: '***'"), "{config_text}");
    assert!(config_text.contains("password: '***'"), "{config_text}");
    assert!(config_text.contains("api_key: '***'"), "{config_text}");
    assert!(
        config_text.contains("endpoint: probe.example"),
        "{config_text}"
    );
    assert!(
        config_text.contains("dsn: '***'"),
        "the schema of a disabled product still masks its section: {config_text}"
    );
    if let Some((_, rest)) = site.config.database.expose().split_once("://")
        && let Some((userinfo, _)) = rest.split_once('@')
        && let Some((_, password)) = userinfo.split_once(':')
    {
        assert!(
            !config_text.contains(&format!(":{password}@")),
            "the database password is redacted: {config_text}"
        );
        assert!(config_text.contains(":***@"), "{config_text}");
        for (name, bytes) in &entries {
            assert!(
                !contains(bytes, &format!(":{password}@")),
                "{name} carries the database password"
            );
        }
    }
    let audit = String::from_utf8_lossy(entry(&entries, "audit.jsonl")).into_owned();
    assert!(audit.contains("notify.channel.set"), "{audit}");
    assert!(
        audit.contains("\"password\": \"***\"") || audit.contains("\"password\":\"***\""),
        "{audit}"
    );
    assert!(audit.contains("auth.token.create"), "{audit}");
    let logs = String::from_utf8_lossy(entry(&entries, "logs.jsonl")).into_owned();
    assert!(
        logs.contains("the probe token was created"),
        "the ring of the running service is in the bundle: {logs}"
    );
    let manifest: Value = serde_json::from_slice(entry(&entries, "manifest.json")).unwrap();
    assert_eq!(manifest["keel"], json!(keel::VERSION));
    assert!(manifest["instance"].is_string(), "{manifest}");
    assert!(
        manifest["notes"]["logs.jsonl"]
            .as_str()
            .unwrap_or_default()
            .contains("entries from the ring"),
        "{manifest}"
    );
    let readiness: Value = serde_json::from_slice(entry(&entries, "readiness.json")).unwrap();
    assert_eq!(readiness["status"], json!(200), "{readiness}");
    assert_eq!(readiness["body"]["status"], json!("ready"), "{readiness}");
    let ledgers: Value = serde_json::from_slice(entry(&entries, "ledgers.json")).unwrap();
    assert!(
        ledgers
            .as_array()
            .expect("ledgers")
            .iter()
            .any(|ledger| ledger["module"] == json!("keel") && ledger["present"] == json!(true)),
        "{ledgers}"
    );
    let doctor_text = String::from_utf8_lossy(entry(&entries, "doctor.txt")).into_owned();
    assert!(doctor_text.contains("0 fail"), "{doctor_text}");
    let problems: Value = serde_json::from_slice(entry(&entries, "problems.json")).unwrap();
    assert!(problems.is_array(), "{problems}");
    running.stop().await.expect("the site stops");
    site.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn logs_tail_answers_the_newest_entries_in_order() {
    let site = site().await;
    init_logging(&site.config);
    let runtime = site.runtime();
    let running = runtime.start(&site.config).await.expect("the site starts");
    let marker = format!("tail-probe-{}", Id::generate());
    for index in 1..=3 {
        tracing::info!(index, "{marker} {index}");
    }
    tracing::warn!("{marker} warned");
    let tail = owner_call(&running, "keel.logs.tail", json!({ "limit": 10_000 }), None).await;
    let entries = tail["entries"].as_array().expect("entries");
    assert!(entries.len() >= 4, "{tail}");
    let seqs: Vec<u64> = entries
        .iter()
        .map(|entry| entry["seq"].as_u64().unwrap_or_default())
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "the tail is in order of arrival: {seqs:?}"
    );
    let marked: Vec<&Value> = entries
        .iter()
        .filter(|entry| {
            entry["message"]
                .as_str()
                .is_some_and(|message| message.starts_with(&marker))
        })
        .collect();
    let messages: Vec<&str> = marked
        .iter()
        .map(|entry| entry["message"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        messages,
        [
            format!("{marker} 1"),
            format!("{marker} 2"),
            format!("{marker} 3"),
            format!("{marker} warned"),
        ]
    );
    assert_eq!(marked[0]["fields"]["index"], json!(1));
    assert_eq!(marked[0]["level"], json!("info"));
    assert_eq!(marked[3]["level"], json!("warn"));
    assert!(
        marked[0]["target"]
            .as_str()
            .unwrap_or_default()
            .starts_with("doctor")
    );
    assert!(tail["total"].as_u64().unwrap_or_default() >= 4);
    assert_eq!(tail["capacity"], json!(10_000));
    let last_seq = seqs.last().copied().unwrap_or_default();
    let newest = owner_call(&running, "keel.logs.tail", json!({ "limit": 1 }), None).await;
    let newest = newest["entries"].as_array().expect("entries");
    assert_eq!(newest.len(), 1);
    assert!(
        newest[0]["seq"].as_u64().unwrap_or_default() >= last_seq,
        "limit 1 answers the newest entry: {newest:?}"
    );
    let two = owner_call(&running, "keel.logs.tail", json!({ "limit": 2 }), None).await;
    let two = two["entries"].as_array().expect("entries");
    assert_eq!(two.len(), 2);
    assert!(two[0]["seq"].as_u64() < two[1]["seq"].as_u64());
    let warnings = owner_call(
        &running,
        "keel.logs.tail",
        json!({ "level": "warn", "limit": 10_000 }),
        None,
    )
    .await;
    let warned = warnings["entries"].as_array().expect("entries");
    assert!(
        warned
            .iter()
            .all(|entry| entry["level"] == json!("warn") || entry["level"] == json!("error")),
        "{warnings}"
    );
    assert!(
        warned
            .iter()
            .any(|entry| entry["message"] == json!(format!("{marker} warned"))),
        "{warnings}"
    );
    let default_limit = owner_call(&running, "keel.logs.tail", json!({}), None).await;
    assert!(default_limit["entries"].as_array().expect("entries").len() <= 200);
    let mut client = Client::connect(running.socket()).await.expect("the socket");
    let refused = client
        .call("keel.logs.tail", json!({ "limit": 0 }), Default::default())
        .await
        .expect("an answer");
    assert!(
        matches!(refused, Envelope::Err { .. }),
        "a limit below one is refused by the schema: {refused:?}"
    );
    running.stop().await.expect("the site stops");
    site.finish().await;
}
