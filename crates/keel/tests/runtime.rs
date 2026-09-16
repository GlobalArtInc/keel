use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use keel::sdk::prelude::*;
use keel::sdk::system::SecretCipher;
use keel::sdk::{Registrar, migrator};
use keel::types::{Envelope, ErrorCode};
use keel::{Change, Config, Error, ExitCode, Frontend, Running, Runtime, TlsMaterial};
use keel_kernel::ctl::{self, Out};
use keel_kernel::dispatcher::CallOptions;
use keel_kernel::secret::{DataKeyCipher, Key, Sealed};
use keel_kernel::socket::{Client, OWNER_LOGIN};
use keel_testkit::TestDatabase;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pong {
    login: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Plain {
    value: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Round {
    sealed: String,
    opened: String,
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
        function::<Nothing, Pong>("probe.ping")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Pong {
                    login: call.principal().login.clone(),
                })
            }),
    )
    .function(
        function::<Plain, Round>("probe.seal")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Plain>| async move {
                let cipher = call.require::<dyn SecretCipher>()?;
                let sealed = cipher.seal(call.params().value.as_bytes())?;
                let opened = cipher.open(&sealed)?;
                Ok(Round {
                    sealed,
                    opened: String::from_utf8_lossy(&opened).into_owned(),
                })
            }),
    )
    .require::<dyn SecretCipher>();
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
    Site {
        database,
        dir,
        config,
        config_path,
    }
}

impl Site {
    fn runtime(&self) -> Runtime {
        Runtime::new().product(&PROBE)
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

    async fn cli(&self, rest: &[&str]) -> ExitCode {
        let runtime = self.runtime();
        let args = self.args(rest);
        tokio::task::spawn_blocking(move || runtime.run_from(args))
            .await
            .expect("the command finishes")
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

async fn http_fetch(running: &Running, path: &str) -> (u16, reqwest::header::HeaderMap, String) {
    let url = running.url(path);
    assert!(url.starts_with("https://"), "{url}");
    let response = client(running.tls())
        .get(url)
        .header("host", "keel.test")
        .send()
        .await
        .expect("the listener answers");
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.text().await.expect("the body is read");
    (status, headers, body)
}

async fn http_get(running: &Running, path: &str) -> (u16, Value) {
    let (status, _, body) = http_fetch(running, path).await;
    (status, serde_json::from_str(&body).expect("a JSON body"))
}

fn header_of<'a>(head: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    head.get(name).and_then(|value| value.to_str().ok())
}

#[tokio::test]
async fn a_runtime_of_the_bare_kernel_with_no_products_starts_on_a_temporary_socket_and_answers_keel_account_list()
 {
    let site = site().await;
    let running = Runtime::new()
        .start(&site.config)
        .await
        .expect("the bare kernel starts");
    assert_eq!(running.products(), &[] as &[String]);
    assert_eq!(running.modules(), ["keel"]);
    assert_eq!(running.socket(), site.config.socket);
    assert_ne!(running.http_addr().port(), 0);
    assert!(site.config.data_dir.join("secret.key").exists());
    assert!(site.config.data_dir.join("instance.id").exists());
    let mut client = Client::connect(running.socket())
        .await
        .expect("the socket accepts the kernel's own uid");
    let envelope = client
        .call("keel.account.list", json!({}), CallOptions::default())
        .await
        .expect("the socket answers");
    let Envelope::Ok { data, meta } = envelope else {
        panic!("expected an ok envelope: {envelope:?}");
    };
    assert_eq!(data["total"], json!(1));
    assert_eq!(data["data"][0]["kind"], json!("system"));
    let principal = meta.principal.expect("the owner");
    assert_eq!(principal.level, Level::Owner);
    assert_eq!(principal.login, OWNER_LOGIN);
    assert_eq!(principal.account, Id::ROOT_ACCOUNT);
    let (status, body) = http_get(&running, "/readyz").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({ "status": "ready" }));
    let (status, body) = http_get(&running, "/api/v1/call/keel.account.list").await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], json!("unauthenticated"));
    let addr = running.http_addr();
    running.stop().await.expect("the runtime stops");
    assert!(!site.config.socket.exists());
    assert!(TcpStream::connect(addr).await.is_err());
    site.finish().await;
}

#[tokio::test]
async fn the_runtime_listens_on_tls_alone_and_a_plain_request_gets_no_http_answer() {
    let site = site().await;
    let running = Runtime::new()
        .start(&site.config)
        .await
        .expect("the bare kernel starts");
    let tls_dir = site.config.data_dir.join("tls");
    let stored = std::fs::read_to_string(tls_dir.join("cert.pem")).expect("cert.pem is written");
    assert_eq!(stored, running.tls().certificate_pem());
    assert!(tls_dir.join("key.pem").is_file());
    assert_eq!(
        running.url("/healthz"),
        format!("https://{}/healthz", running.http_addr())
    );

    let (status, body) = http_get(&running, "/healthz").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({ "status": "ok" }));

    let other = TlsMaterial::generate(&keel_kernel::tls::Names::for_host(
        None,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    ))
    .expect("another pair");
    let refused = client(&other).get(running.url("/healthz")).send().await;
    let error = refused.expect_err("a client that does not trust the certificate is refused");
    assert!(error.is_connect() || error.is_request(), "{error}");

    let mut plain = TcpStream::connect(running.http_addr())
        .await
        .expect("the port accepts a connection");
    plain
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: keel.test\r\nConnection: close\r\n\r\n")
        .await
        .expect("the plain request is written");
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(HTTP_TIMEOUT, plain.read_to_end(&mut answer)).await;
    let text = String::from_utf8_lossy(&answer);
    assert!(
        !text.starts_with("HTTP/"),
        "a plain request gets no HTTP answer: {text:?}"
    );

    let fingerprint = running.tls().fingerprint();
    assert_eq!(fingerprint.len(), 95, "{fingerprint}");
    running.stop().await.expect("the runtime stops");
    let again = Runtime::new()
        .start(&site.config)
        .await
        .expect("the bare kernel starts again");
    assert_eq!(
        again.tls().fingerprint(),
        fingerprint,
        "the stored pair is reused at the next start"
    );
    again.stop().await.expect("the runtime stops");
    site.finish().await;
}

static PROVIDER_SEEN_AT_START: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

static WATCHER: ModuleDef = ModuleDef {
    id: "watcher",
    version: "0.1.0",
    sdk: keel::sdk::VERSION,
    requires: &[],
    migrations: None,
    init: watcher_init,
};

fn watcher_init(r: &mut Registrar) -> Result<()> {
    r.start(|_| async {
        PROVIDER_SEEN_AT_START.store(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            std::sync::atomic::Ordering::SeqCst,
        );
        Ok(())
    });
    Ok(())
}

#[tokio::test]
async fn the_runtime_installs_the_ring_provider_before_the_modules_start() {
    let site = site().await;
    let running = Runtime::new()
        .module(&WATCHER)
        .start(&site.config)
        .await
        .expect("the kernel starts with the watcher");
    assert!(
        PROVIDER_SEEN_AT_START.load(std::sync::atomic::Ordering::SeqCst),
        "the watcher module saw a process-level TLS provider in its start hook"
    );
    let provider = rustls::crypto::CryptoProvider::get_default().expect("a provider is installed");
    assert!(
        std::ptr::eq(
            provider.secure_random,
            rustls::crypto::ring::default_provider().secure_random
        ),
        "the installed provider is ring"
    );
    running.stop().await.expect("the runtime stops");
    site.finish().await;
}

#[tokio::test]
async fn a_runtime_with_an_embedded_frontend_serves_the_index_and_its_assets_beside_the_api() {
    let site = site().await;
    let frontend: Frontend = keel::embed_frontend!("tests/frontend");
    let running = Runtime::new()
        .frontend(frontend)
        .start(&site.config)
        .await
        .expect("the kernel with a frontend starts");
    let (status, head, body) = http_fetch(&running, "/").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, include_str!("frontend/index.html"));
    assert_eq!(header_of(&head, "cache-control"), Some("no-store"));
    assert!(
        header_of(&head, "content-security-policy")
            .is_some_and(|policy| policy.starts_with("default-src 'self'")),
        "{head:?}"
    );
    let (status, _, screen) = http_fetch(&running, "/f/keel.account.list").await;
    assert_eq!(status, 200);
    assert_eq!(screen, body);
    let (status, head, script) = http_fetch(&running, "/main-abc123.js").await;
    assert_eq!(status, 200, "{script}");
    assert_eq!(script, include_str!("frontend/main-abc123.js"));
    assert_eq!(
        header_of(&head, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let (status, body) = http_get(&running, "/missing.png").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));
    let (status, body) = http_get(&running, "/api/v1/call/keel.account.list").await;
    assert_eq!(status, 401, "{body}");
    let (status, body) = http_get(&running, "/readyz").await;
    assert_eq!(status, 200, "{body}");
    running.stop().await.expect("the runtime stops");
    site.finish().await;
}

#[tokio::test]
async fn serve_runs_until_the_shutdown_future_resolves() {
    let site = site().await;
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let runtime = Runtime::new();
    let config = site.config.clone();
    let served = tokio::spawn(async move {
        runtime
            .serve(&config, async move {
                let _ = rx.await;
            })
            .await
    });
    let socket = site.config.socket.clone();
    let mut attempts = 0;
    let mut client = loop {
        match Client::connect(&socket).await {
            Ok(client) => break client,
            Err(_) if attempts < 100 => {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => panic!("the socket never came up: {error}"),
        }
    };
    let answer = client
        .call("keel.account.list", json!({}), CallOptions::default())
        .await
        .expect("the socket answers");
    assert!(matches!(answer, Envelope::Ok { .. }), "{answer:?}");
    tx.send(()).expect("serve is waiting");
    served
        .await
        .expect("serve finishes")
        .expect("serve stops cleanly");
    assert!(!socket.exists());
    site.finish().await;
}

#[tokio::test]
async fn product_enable_of_an_unknown_id_fails_naming_the_known_ids_and_list_prints_the_enabled_set()
 {
    let site = site().await;
    let runtime = site.runtime();
    let error = runtime
        .enable_product(&site.config, "x")
        .await
        .expect_err("an unknown bundle is refused");
    assert!(matches!(error, Error::UnknownProduct { .. }), "{error}");
    assert_eq!(
        error.to_string(),
        "the product x is not carried by this binary; it carries probe"
    );
    let bare = Runtime::new()
        .enable_product(&site.config, "probe")
        .await
        .expect_err("a runtime without bundles carries nothing");
    assert_eq!(
        bare.to_string(),
        "the product probe is not carried by this binary; it carries no product bundle"
    );
    let states = runtime
        .list_products(&site.config)
        .await
        .expect("the list reads the database");
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].id, "probe");
    assert!(!states[0].enabled);
    assert_eq!(states[0].version, Some("0.2.0"));

    assert_eq!(
        runtime.enable_product(&site.config, "probe").await.unwrap(),
        Change::Enabled
    );
    assert_eq!(
        runtime.enable_product(&site.config, "probe").await.unwrap(),
        Change::AlreadyEnabled
    );
    let states = runtime.list_products(&site.config).await.unwrap();
    assert!(states[0].enabled);
    assert_eq!(states[0].to_string(), "probe\tenabled\t0.2.0");
    let stored: Value = serde_json::from_str(
        &site
            .database
            .rows("SELECT value::text FROM keel_kv WHERE key = 'products'")
            .await[0],
    )
    .unwrap();
    assert_eq!(stored["probe"]["by"], json!("install"));
    assert!(
        stored["probe"]["enabledAt"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z') && at.len() >= 20),
        "{stored}"
    );
    assert_eq!(stored.as_object().map(|set| set.len()), Some(1));

    let bare_states = Runtime::new().list_products(&site.config).await.unwrap();
    assert_eq!(bare_states.len(), 1);
    assert!(bare_states[0].enabled);
    assert_eq!(bare_states[0].version, None);
    assert_eq!(
        bare_states[0].to_string(),
        "probe\tenabled\tnot carried by this binary"
    );
    let failed = Runtime::new()
        .start(&site.config)
        .await
        .expect_err("a binary without the enabled bundle does not start");
    assert!(matches!(failed, Error::UnknownProduct { .. }), "{failed}");

    assert_eq!(
        runtime
            .disable_product(&site.config, "probe")
            .await
            .unwrap(),
        Change::Disabled
    );
    assert_eq!(
        runtime
            .disable_product(&site.config, "probe")
            .await
            .unwrap(),
        Change::AlreadyDisabled
    );
    let unknown = runtime
        .disable_product(&site.config, "x")
        .await
        .expect_err("an unknown bundle is refused");
    assert!(matches!(unknown, Error::UnknownProduct { .. }), "{unknown}");
    assert_eq!(
        Runtime::new()
            .disable_product(&site.config, "probe")
            .await
            .expect_err("nothing to disable")
            .to_string(),
        "the product probe is not carried by this binary; it carries no product bundle"
    );

    assert_eq!(
        site.cli(&["product", "enable", "x"]).await,
        ExitCode::FAILURE
    );
    assert_eq!(
        site.cli(&["product", "enable", "probe"]).await,
        ExitCode::SUCCESS
    );
    assert_eq!(site.cli(&["product", "list"]).await, ExitCode::SUCCESS);
    assert_eq!(
        site.cli(&["product", "disable", "probe"]).await,
        ExitCode::SUCCESS
    );
    site.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_enabled_bundle_loads_its_modules_and_a_disabled_one_leaves_them_orphaned() {
    let site = site().await;
    let runtime = site.runtime();
    let migrated = runtime.migrate(&site.config).await.expect("migrate runs");
    assert_eq!(migrated.modules.len(), 1);
    assert_eq!(migrated.modules[0].0, "keel");
    assert!(migrated.orphaned.is_empty());

    runtime.enable_product(&site.config, "probe").await.unwrap();
    let migrated = runtime.migrate(&site.config).await.expect("migrate runs");
    assert_eq!(migrated.modules[1].0, "probe");
    assert_eq!(migrated.modules[1].1.len(), 1, "{migrated:?}");
    assert_eq!(
        site.database
            .rows("SELECT id FROM keel_module WHERE state = 'active' ORDER BY id")
            .await,
        ["keel", "probe"]
    );
    assert_eq!(
        site.database
            .rows("SELECT table_name::text FROM information_schema.tables WHERE table_name = 'probe_item'")
            .await,
        ["probe_item"]
    );

    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts");
    assert_eq!(running.products(), ["probe"]);
    assert_eq!(running.modules(), ["keel", "probe"]);
    let mut client = Client::connect(running.socket()).await.unwrap();
    let answer = client
        .call("probe.ping", json!({}), CallOptions::default())
        .await
        .unwrap();
    let Envelope::Ok { data, .. } = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(data["login"], json!(OWNER_LOGIN));
    let refused = runtime
        .disable_product(&site.config, "probe")
        .await
        .expect_err("the enabled set is not edited while the service runs");
    assert!(matches!(refused, Error::ServiceRunning { .. }), "{refused}");
    assert_eq!(
        site.cli(&["product", "disable", "probe"]).await,
        ExitCode::FAILURE
    );
    assert_eq!(
        site.cli(&["ctl", "probe.ping", "--out", "table"]).await,
        ExitCode::SUCCESS
    );
    assert_eq!(
        site.cli(&["ctl", "keel.account.create", "kind=user", "--out", "json"])
            .await,
        ExitCode::SUCCESS
    );
    assert_eq!(
        site.cli(&["ctl", "keel.account.create", "kind=not a name"])
            .await,
        ExitCode::FAILURE
    );
    assert_eq!(
        site.cli(&["ctl", "probe.ping", "broken"]).await,
        ExitCode::FAILURE
    );
    running.stop().await.expect("the runtime stops");

    runtime
        .disable_product(&site.config, "probe")
        .await
        .unwrap();
    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts");
    assert_eq!(running.modules(), ["keel"]);
    let mut client = Client::connect(running.socket()).await.unwrap();
    let answer = client
        .call("probe.ping", json!({}), CallOptions::default())
        .await
        .unwrap();
    let Envelope::Err { error } = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(error.code, ErrorCode::NotFound);
    running.stop().await.expect("the runtime stops");
    assert_eq!(
        site.database
            .rows("SELECT state FROM keel_module WHERE id = 'probe'")
            .await,
        ["orphaned"]
    );
    assert_eq!(
        site.database
            .rows("SELECT table_name::text FROM information_schema.tables WHERE table_name = 'probe_item'")
            .await,
        ["probe_item"]
    );
    assert_eq!(
        site.cli(&[
            "ctl",
            "keel.account.list",
            "--socket",
            &site.config.socket.display().to_string()
        ])
        .await,
        ExitCode::FAILURE
    );
    site.finish().await;
}

#[tokio::test]
async fn the_runtime_provides_the_cipher_over_the_data_key_the_instance_keeps_in_keel_kv() {
    let site = site().await;
    let runtime = site.runtime();
    runtime.enable_product(&site.config, "probe").await.unwrap();
    let running = runtime
        .start(&site.config)
        .await
        .expect("the runtime starts");
    let mut client = Client::connect(running.socket()).await.unwrap();
    let answer = client
        .call(
            "probe.seal",
            json!({ "value": "JBSWY3DPEHPK3PXP" }),
            CallOptions::default(),
        )
        .await
        .unwrap();
    let Envelope::Ok { data, .. } = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(data["opened"], json!("JBSWY3DPEHPK3PXP"));
    let sealed = data["sealed"].as_str().expect("the sealed text").to_owned();
    assert!(!sealed.contains("JBSWY3DPEHPK3PXP"), "{sealed}");
    running.stop().await.expect("the runtime stops");

    let file_key = Key::parse_hex(
        &std::fs::read_to_string(site.config.data_dir.join("secret.key"))
            .expect("the secret key file"),
    )
    .expect("a key");
    let wrapped = site
        .database
        .rows("SELECT value::text FROM keel_kv WHERE key = 'secret_key'")
        .await;
    let wrapped: Sealed = serde_json::from_str(&wrapped[0]).expect("the wrapped data key");
    let data_key = Key::from_slice(&file_key.open(&wrapped).expect("the file key opens it"))
        .expect("a data key");
    assert_eq!(
        DataKeyCipher::new(data_key).open(&sealed).expect("opens"),
        b"JBSWY3DPEHPK3PXP"
    );
    assert_eq!(
        DataKeyCipher::new(file_key)
            .open(&sealed)
            .unwrap_err()
            .key(),
        "keel.error.secret_open"
    );
    site.finish().await;
}

#[tokio::test]
async fn ctl_keel_account_create_returns_the_envelope_and_out_table_renders_the_columns() {
    let site = site().await;
    let running = Runtime::new().start(&site.config).await.unwrap();
    let created = ctl::run(
        running.socket(),
        &ctl::Request {
            func: "keel.account.create".into(),
            params: ctl::params(["kind=reseller"]).unwrap(),
            out: Out::Json,
            lang: "en".into(),
            run_as: None,
        },
    )
    .await
    .unwrap();
    assert!(created.ok, "{}", created.text);
    let envelope: Value = serde_json::from_str(&created.text).unwrap();
    assert_eq!(envelope["ok"], json!(true));
    let id = envelope["data"]["id"].as_str().expect("the new id");
    assert!(id.parse::<Id>().is_ok(), "{id}");
    let table = ctl::run(
        running.socket(),
        &ctl::Request {
            func: "keel.account.list".into(),
            params: json!({}),
            out: Out::Table,
            lang: "en".into(),
            run_as: None,
        },
    )
    .await
    .unwrap();
    assert!(table.ok, "{}", table.text);
    let lines: Vec<&str> = table.text.lines().collect();
    assert_eq!(lines.len(), 3, "{}", table.text);
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        ["Account", "Kind", "State", "Parent", "Created", "Origin"]
    );
    assert!(
        lines.iter().any(|line| line.starts_with(id)),
        "{}",
        table.text
    );
    running.stop().await.unwrap();
    site.finish().await;
}
