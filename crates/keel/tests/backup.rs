use std::collections::BTreeSet;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use keel::sdk::prelude::*;
use keel::sdk::sea_orm::EntityTrait;
use keel::sdk::system::BackupManifest;
use keel::sdk::{Registrar, migrator};
use keel::types::Envelope;
use keel::{Config, ExitCode, Running, Runtime, TlsMaterial};
use keel_kernel::dispatcher::CallOptions;
use keel_kernel::socket::Client;
use keel_system::auth::{decode_totp_secret, totp_code, totp_step};
use keel_testkit::TestDatabase;
use tempfile::TempDir;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PASSWORD: &str = "correct-horse-battery";
const PASSPHRASE: &str = "the passphrase of the rehearsal";
const SOURCE_FILE: &str = "notes.txt";
const RESTORED_FILE: &str = "probe-restored.txt";

#[derive(Deserialize, Serialize, JsonSchema)]
struct Title {
    title: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Saved {
    id: String,
}

mod item {
    use keel::sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "probe_item")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
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
                let row = item::ActiveModel {
                    id: keel::sdk::sea_orm::ActiveValue::Set(id.clone()),
                    account_id: keel::sdk::sea_orm::ActiveValue::Set(
                        call.principal().account_id.to_string(),
                    ),
                    title: keel::sdk::sea_orm::ActiveValue::Set(call.params().title.clone()),
                };
                item::Entity::insert(row).exec(call.tx()?).await?;
                call.emit("probe.item.created", &json!({ "id": id }))?;
                Ok(Saved { id })
            }),
    )
    .contribute(
        BackupSource::new("probe.notes", |dir: BackupDir| async move {
            let rows = item::Entity::find().all(dir.db()).await?;
            let mut titles: Vec<String> = rows.into_iter().map(|row| row.title).collect();
            titles.sort();
            std::fs::write(dir.path().join(SOURCE_FILE), titles.join("\n"))
                .map_err(|error| KeelError::internal("probe.error.source").source(error))?;
            Ok(())
        })
        .restore(|dir: BackupDir| async move {
            let text = std::fs::read_to_string(dir.path().join(SOURCE_FILE))
                .map_err(|error| KeelError::internal("probe.error.source").source(error))?;
            std::fs::write(dir.data_dir().join(RESTORED_FILE), text)
                .map_err(|error| KeelError::internal("probe.error.source").source(error))?;
            Ok(())
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
    let etc = dir.path().join("etc");
    std::fs::create_dir_all(etc.join("templates")).expect("the config directory");
    std::fs::write(etc.join("templates").join("zone.j2"), "zone template\n")
        .expect("a template override");
    let config_path = etc.join("config.yaml");
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
            .module(&keel_system::auth::MODULE)
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

    fn archive(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    async fn finish(self) {
        drop(self.dir);
        self.database.finish().await;
    }
}

fn client(trusting: &TlsMaterial) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([
            reqwest::Certificate::from_pem(trusting.certificate_pem().as_bytes())
                .expect("the certificate parses"),
        ])
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("a client")
}

async fn post(
    running: &Running,
    path: &str,
    body: Value,
    cookie: Option<&str>,
) -> (u16, Option<String>, Value) {
    let mut request = client(running.tls())
        .post(running.url(path))
        .header("host", "keel.test")
        .header("content-type", "application/json")
        .body(body.to_string());
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("the listener answers");
    let status = response.status().as_u16();
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .filter(|value| value.starts_with(keel_kernel::http::SESSION_COOKIE))
        .map(str::to_owned);
    let text = response.text().await.expect("the body is read");
    (
        status,
        cookie,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn get(running: &Running, path: &str) -> (u16, Value) {
    let response = client(running.tls())
        .get(running.url(path))
        .header("host", "keel.test")
        .send()
        .await
        .expect("the listener answers");
    let status = response.status().as_u16();
    let text = response.text().await.expect("the body is read");
    (status, serde_json::from_str(&text).expect("a JSON body"))
}

async fn owner_call(running: &Running, func: &str, params: Value, run_as: Option<&str>) -> Value {
    let mut client = Client::connect(running.socket())
        .await
        .expect("the socket accepts the owner");
    let envelope = client
        .call_as(func, params, run_as, CallOptions::default())
        .await
        .expect("the socket answers");
    match envelope {
        Envelope::Ok { data, .. } => data,
        Envelope::Err { error } => panic!("{func} failed: {error:?}"),
    }
}

struct Enrolled {
    secret: Vec<u8>,
}

async fn enrol_alice(running: &Running) -> Enrolled {
    let account = owner_call(
        running,
        "keel.account.create",
        json!({ "kind": "user" }),
        None,
    )
    .await;
    let principal = owner_call(
        running,
        "keel.principal.create",
        json!({ "login": "alice", "level": "user", "account": account["id"] }),
        None,
    )
    .await;
    owner_call(
        running,
        "auth.password.set",
        json!({ "principal": principal["id"], "password": PASSWORD }),
        None,
    )
    .await;
    for title in ["alpha", "beta"] {
        owner_call(
            running,
            "probe.item.create",
            json!({ "title": title }),
            Some("alice"),
        )
        .await;
    }
    let enrolled = owner_call(running, "auth.totp.enroll", json!({}), Some("alice")).await;
    let secret =
        decode_totp_secret(enrolled["secret"].as_str().expect("a secret")).expect("base32");
    let code = totp_code(&secret, totp_step(keel::sdk::chrono::Utc::now()) - 1).expect("a code");
    owner_call(
        running,
        "auth.totp.confirm",
        json!({ "code": code }),
        Some("alice"),
    )
    .await;
    Enrolled { secret }
}

async fn login_with_totp(
    running: &Running,
    secret: &[u8],
    step_offset: i64,
) -> (u16, Option<String>, Value) {
    let (status, cookie, body) = post(
        running,
        "/api/v1/call/auth.login",
        json!({ "login": "alice", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(cookie.is_none(), "no session before the second factor");
    assert_eq!(body["data"]["state"], json!("totp"), "{body}");
    let challenge = body["data"]["challenge"]
        .as_str()
        .expect("a challenge")
        .to_owned();
    let code = totp_code(
        secret,
        totp_step(keel::sdk::chrono::Utc::now()) + step_offset,
    )
    .expect("a code");
    post(
        running,
        "/api/v1/call/auth.totp.verify",
        json!({ "challenge": challenge, "code": code }),
        None,
    )
    .await
}

fn entries_of(archive: &Path) -> (BTreeSet<String>, BackupManifest) {
    let file = std::fs::File::open(archive).expect("the archive opens");
    let decoder = zstd::Decoder::new(file).expect("a zstd stream");
    let mut tar = tar::Archive::new(decoder);
    let mut names = BTreeSet::new();
    let mut manifest = None;
    for entry in tar.entries().expect("entries") {
        let mut entry = entry.expect("an entry");
        let path = entry.path().expect("a path").display().to_string();
        if path == "manifest.json" {
            let mut text = String::new();
            entry.read_to_string(&mut text).expect("the manifest reads");
            manifest = Some(serde_json::from_str(&text).expect("the manifest parses"));
        }
        names.insert(path);
    }
    (names, manifest.expect("a manifest"))
}

async fn boot_of(running: &Running) -> Value {
    let (status, body) = get(running, "/api/v1/boot").await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn titles_of(site: &Site) -> Vec<String> {
    site.database
        .rows("SELECT title FROM probe_item ORDER BY title")
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_restored_into_a_fresh_database_and_data_dir_answers_the_same_boot_and_logs_in_with_the_same_totp()
 {
    let source = site().await;
    let runtime = source.runtime();
    assert_eq!(
        source
            .cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&source.config)
        .await
        .expect("the source starts");
    let enrolled = enrol_alice(&running).await;
    let (status, cookie, body) = login_with_totp(&running, &enrolled.secret, 0).await;
    assert_eq!(status, 200, "{body}");
    assert!(cookie.is_some(), "the source logs alice in: {body}");
    let source_boot = boot_of(&running).await;
    let archive = source.archive("site.tar.zst");
    assert_eq!(
        source
            .cli(
                runtime.clone(),
                &[
                    "backup",
                    "--out",
                    archive.to_str().expect("utf-8"),
                    "--with-secret-key",
                    "--passphrase",
                    PASSPHRASE,
                ],
            )
            .await,
        ExitCode::SUCCESS
    );
    running.stop().await.expect("the source stops");
    let (entries, manifest) = entries_of(&archive);
    assert!(manifest.secret_key, "{manifest:?}");
    assert_eq!(manifest.keel, keel::VERSION);
    assert_eq!(
        manifest
            .products
            .iter()
            .map(|product| (
                product.id.as_str(),
                product.version.as_str(),
                product.edition.as_deref()
            ))
            .collect::<Vec<_>>(),
        [("probe", "0.2.0", Some("lite"))]
    );
    let modules: Vec<(&str, u32)> = manifest
        .modules
        .iter()
        .map(|module| (module.id.as_str(), module.applied))
        .collect();
    assert!(modules.contains(&("keel", 2)), "{modules:?}");
    assert!(modules.contains(&("probe", 1)), "{modules:?}");
    assert!(
        modules
            .iter()
            .any(|(id, applied)| *id == "auth" && *applied > 5),
        "{modules:?}"
    );
    assert!(
        manifest
            .tables
            .iter()
            .any(|table| table.name == "auth_totp"
                && table.columns.contains(&"principal_id".to_owned())),
        "{:?}",
        manifest.tables
    );
    assert_eq!(manifest.config, ["config.yaml", "templates/zone.j2"]);
    assert!(manifest.data.contains(&"instance.id".to_owned()));
    assert!(manifest.data.contains(&"tls/key.pem".to_owned()));
    assert!(manifest.data.contains(&"secret.key.sealed".to_owned()));
    assert!(manifest.instance.is_some());
    assert!(entries.contains("db/auth_totp.copy"), "{entries:?}");
    assert!(entries.contains("etc/config.yaml"), "{entries:?}");
    assert!(entries.contains("data/secret.key.sealed"), "{entries:?}");
    assert!(!entries.contains("data/secret.key"), "{entries:?}");
    let sealed_names: Vec<&String> = entries
        .iter()
        .filter(|name| name.contains("lib/"))
        .collect();
    assert!(
        sealed_names.is_empty(),
        "the data directory is not under etc: {sealed_names:?}"
    );

    let target = site().await;
    assert_eq!(
        target
            .cli(
                runtime.clone(),
                &[
                    "restore",
                    archive.to_str().expect("utf-8"),
                    "--with-secret-key",
                    "--passphrase",
                    PASSPHRASE,
                ],
            )
            .await,
        ExitCode::SUCCESS
    );
    assert_eq!(
        std::fs::read_to_string(target.config.data_dir.join("secret.key")).unwrap(),
        std::fs::read_to_string(source.config.data_dir.join("secret.key")).unwrap(),
        "the secret key travels sealed and lands in the clear"
    );
    assert_eq!(
        std::fs::read_to_string(target.config.data_dir.join("instance.id")).unwrap(),
        std::fs::read_to_string(source.config.data_dir.join("instance.id")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(target.config.data_dir.join("tls").join("cert.pem")).unwrap(),
        std::fs::read_to_string(source.config.data_dir.join("tls").join("cert.pem")).unwrap()
    );
    assert!(
        target.config_path.with_extension("yaml.restored").exists(),
        "the archive's config lands beside the kept one"
    );
    assert!(
        target
            .dir
            .path()
            .join("etc")
            .join("templates")
            .join("zone.j2")
            .exists()
    );
    assert!(
        std::fs::read_to_string(&target.config_path)
            .unwrap()
            .contains(target.database.url()),
        "the target keeps the configuration it was restored with"
    );
    assert!(target.config.data_dir.join("restore.note").exists());
    assert_eq!(titles_of(&target).await, ["alpha", "beta"]);
    assert_eq!(
        target
            .database
            .rows("SELECT value->'probe'->>'by' FROM keel_kv WHERE key = 'products'")
            .await,
        ["install"]
    );

    let restored = runtime
        .start(&target.config)
        .await
        .expect("the target starts");
    assert!(
        !target.config.data_dir.join("restore.note").exists(),
        "the start consumes the note"
    );
    assert_eq!(restored.products(), ["probe"]);
    assert_eq!(boot_of(&restored).await, source_boot);
    let (status, cookie, body) = login_with_totp(&restored, &enrolled.secret, 1).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        cookie.is_some(),
        "the restored site logs alice in with her factor: {body}"
    );
    let (status, _, body) = post(
        &restored,
        "/api/v1/call/keel.account.list",
        json!({}),
        cookie.as_deref(),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let before = target
        .database
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    assert_eq!(
        before,
        ["2"],
        "the two events of the source came back with their sequence"
    );
    owner_call(
        &restored,
        "probe.item.create",
        json!({ "title": "gamma" }),
        Some("alice"),
    )
    .await;
    assert_eq!(
        target
            .database
            .rows("SELECT max(seq)::text FROM keel_outbox")
            .await,
        ["3"],
        "the identity sequence continues after the restored rows"
    );
    assert_eq!(titles_of(&target).await, ["alpha", "beta", "gamma"]);
    restored.stop().await.expect("the target stops");
    target.finish().await;
    source.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_without_the_key_restores_but_the_sealed_values_do_not_open() {
    let source = site().await;
    let runtime = source.runtime();
    assert_eq!(
        source
            .cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&source.config)
        .await
        .expect("the source starts");
    let enrolled = enrol_alice(&running).await;
    let archive = source.archive("plain.tar.zst");
    assert_eq!(
        source
            .cli(
                runtime.clone(),
                &["backup", "--out", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );
    running.stop().await.expect("the source stops");
    let (entries, manifest) = entries_of(&archive);
    assert!(!manifest.secret_key);
    assert!(!entries.contains("data/secret.key.sealed"), "{entries:?}");
    assert!(!entries.contains("data/secret.key"), "{entries:?}");

    let target = site().await;
    assert_eq!(
        target
            .cli(
                runtime.clone(),
                &[
                    "restore",
                    archive.to_str().expect("utf-8"),
                    "--with-secret-key",
                    "--passphrase",
                    PASSPHRASE,
                ],
            )
            .await,
        ExitCode::FAILURE,
        "an archive without the key cannot restore it"
    );
    assert_eq!(
        target
            .cli(
                runtime.clone(),
                &["restore", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );
    assert!(!target.config.data_dir.join("secret.key").exists());
    assert_eq!(
        target
            .database
            .rows("SELECT count(*)::text FROM keel_kv WHERE key = 'secret_key'")
            .await,
        ["0"],
        "the wrapped data key of the dump is dropped when no key opens it"
    );
    let note: Value = serde_json::from_str(
        &std::fs::read_to_string(target.config.data_dir.join("restore.note")).unwrap(),
    )
    .unwrap();
    assert_eq!(note["data_key"], json!("dropped"));
    assert_eq!(titles_of(&target).await, ["alpha", "beta"]);

    let restored = runtime
        .start(&target.config)
        .await
        .expect("the target starts");
    assert_ne!(
        std::fs::read_to_string(target.config.data_dir.join("secret.key")).unwrap(),
        std::fs::read_to_string(source.config.data_dir.join("secret.key")).unwrap(),
        "the target generates a key of its own"
    );
    let (status, cookie, body) = login_with_totp(&restored, &enrolled.secret, 0).await;
    assert_eq!(status, 500, "{body}");
    assert!(cookie.is_none());
    assert_eq!(
        body["error"]["key"],
        json!("keel.error.secret_open"),
        "{body}"
    );
    let (status, _, body) = post(
        &restored,
        "/api/v1/call/auth.login",
        json!({ "login": "alice", "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["data"]["state"],
        json!("totp"),
        "the hashed password still matches: {body}"
    );
    restored.stop().await.expect("the target stops");
    target.finish().await;
    source.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_refuses_an_archive_naming_a_bundle_the_binary_does_not_carry() {
    let source = site().await;
    let runtime = source.runtime();
    assert_eq!(
        source
            .cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&source.config)
        .await
        .expect("the source starts");
    running.stop().await.expect("the source stops");
    let archive = source.archive("probe.tar.zst");
    assert_eq!(
        source
            .cli(
                runtime.clone(),
                &["backup", "--out", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );
    let (_, manifest) = entries_of(&archive);
    assert_eq!(manifest.products[0].id, "probe");

    let target = site().await;
    let bare = Runtime::new()
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::tasks::MODULE);
    assert_eq!(
        target
            .cli(bare.clone(), &["restore", archive.to_str().expect("utf-8")])
            .await,
        ExitCode::FAILURE
    );
    assert!(
        target
            .database
            .rows("SELECT table_name::text FROM information_schema.tables WHERE table_name = 'probe_item'")
            .await
            .is_empty(),
        "nothing is restored"
    );
    assert!(!target.config.data_dir.join("restore.note").exists());
    assert!(!target.config.data_dir.join("instance.id").exists());
    let leftovers: Vec<_> = std::fs::read_dir(target.config.data_dir.join("backups"))
        .map(|entries| entries.filter_map(|entry| entry.ok()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "no staging remains: {leftovers:?}");
    let restoring = bare
        .start(&target.config)
        .await
        .expect("the bare runtime starts");
    restoring.stop().await.expect("it stops");
    let running = runtime
        .start(&source.config)
        .await
        .expect("the source still starts");
    running.stop().await.expect("the source stops");
    target.finish().await;
    source.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_source_contribution_lands_in_the_archive_and_comes_back() {
    let source = site().await;
    let runtime = source.runtime();
    assert_eq!(
        source
            .cli(runtime.clone(), &["product", "enable", "probe"])
            .await,
        ExitCode::SUCCESS
    );
    let running = runtime
        .start(&source.config)
        .await
        .expect("the source starts");
    enrol_alice(&running).await;
    running.stop().await.expect("the source stops");
    let archive = source.archive("sources.tar.zst");
    assert_eq!(
        source
            .cli(
                runtime.clone(),
                &["backup", "--out", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );
    let (entries, manifest) = entries_of(&archive);
    assert_eq!(manifest.sources, ["probe.notes"]);
    assert!(
        entries.contains(&format!("sources/probe.notes/{SOURCE_FILE}")),
        "{entries:?}"
    );
    assert_eq!(
        source
            .cli(
                runtime.clone(),
                &["backup", "inspect", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );

    let target = site().await;
    assert_eq!(
        target
            .cli(
                runtime.clone(),
                &["restore", archive.to_str().expect("utf-8")]
            )
            .await,
        ExitCode::SUCCESS
    );
    assert_eq!(
        std::fs::read_to_string(target.config.data_dir.join(RESTORED_FILE)).unwrap(),
        "alpha\nbeta"
    );
    let note: Value = serde_json::from_str(
        &std::fs::read_to_string(target.config.data_dir.join("restore.note")).unwrap(),
    )
    .unwrap();
    assert_eq!(note["sources"], json!(["probe.notes"]));
    assert_eq!(note["products_before"], json!([]));
    assert_eq!(note["products_after"], json!(["probe"]));

    let restored = runtime
        .start(&target.config)
        .await
        .expect("the target starts");
    let mut client = Client::connect(restored.socket()).await.unwrap();
    let count = client
        .call("keel.account.list", json!({}), CallOptions::default())
        .await
        .unwrap();
    let Envelope::Ok { data, .. } = count else {
        panic!("{count:?}");
    };
    assert_eq!(data["total"], json!(2), "the root account and alice's");
    let stored = restored.products().to_vec();
    assert_eq!(stored, ["probe"]);
    let kv = target
        .database
        .rows("SELECT key FROM keel_kv ORDER BY key")
        .await;
    assert!(kv.contains(&"instance_id".to_owned()), "{kv:?}");
    assert!(kv.contains(&"secret_key".to_owned()), "{kv:?}");
    restored.stop().await.expect("the target stops");
    target.finish().await;
    source.finish().await;
}
