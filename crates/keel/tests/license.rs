use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use keel::sdk::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use keel::sdk::system::{Degradation, Features};
use keel::types::Envelope;
use keel::types::ErrorCode;
use keel::{Config, ExitCode, Runtime};
use keel_kernel::dispatcher::CallOptions;
use keel_kernel::socket::Client;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::license::{GET, INSTALL, MODULE, PRODUCT_UNLISTED};
use keel_testkit::license::Issuer;
use keel_testkit::{Builder, Harness, Reply, TestDatabase, Who};
use tempfile::TempDir;

const LICENSE: &str = "keel.error.license";
const PATIENCE: Duration = Duration::from_secs(10);
const ENABLE: &str = "keel.product.enable";
const POLL_JOB: &str = "notify.problems.poll";

static ZONES: ModuleDef = ModuleDef {
    id: "zones",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: zones,
};

static SITES: ModuleDef = ModuleDef {
    id: "sites",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: sites,
};

static DNS: ProductManifest = fixture(
    "dns",
    "product.dns.brand",
    &[("en", r#"{ "product": { "dns": { "brand": "DNS" } } }"#)],
    &[&ZONES],
);
static PANEL: ProductManifest = fixture(
    "panel",
    "product.panel.brand",
    &[("en", r#"{ "product": { "panel": { "brand": "Panel" } } }"#)],
    &[&SITES],
);

const fn fixture(
    id: &'static str,
    name: &'static str,
    i18n: &'static [(&'static str, &'static str)],
    modules: &'static [&'static ModuleDef],
) -> ProductManifest {
    ProductManifest {
        id,
        name,
        version: "0.1.0",
        api: keel::types::DESCRIPTOR_VERSION,
        brand: Brand {
            logo: "product.fixture.logo",
            hue: 120,
            chroma: 0.1,
        },
        modules,
        menu: &[],
        levels: LevelLabels {
            labels: &[],
            disabled: &[],
        },
        account_kinds: &["user"],
        editions: &[Edition {
            id: "lite",
            features: &[],
        }],
        i18n,
        assets: &[],
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Note {
    text: String,
}

fn zones(r: &mut Registrar) -> keel::sdk::Result<()> {
    notes(r, "zones")
}

fn sites(r: &mut Registrar) -> keel::sdk::Result<()> {
    notes(r, "sites")
}

fn notes(r: &mut Registrar, module: &'static str) -> keel::sdk::Result<()> {
    let key = format!("{module}_note");
    let read = key.clone();
    r.function(
        function::<Nothing, Note>(&format!("{module}.note.get"))
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .ui(ObjectScreen::new(&format!("{module}.note.title"))
                .action(Action::new("edit", ActionRole::Edit).screen(&format!("{module}.note.set"))))
            .handler(move |call: Call<Nothing>| {
                let key = read.clone();
                async move {
                    let row = call
                        .db()
                        .query_one(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "SELECT value #>> '{}' AS text FROM keel_kv WHERE key = $1",
                            [DbValue::from(key)],
                        ))
                        .await?;
                    let text = match row {
                        Some(row) => row.try_get::<String>("", "text")?,
                        None => String::new(),
                    };
                    Ok(Note { text })
                }
            }),
    )
    .function(
        function::<Note, Note>(&format!("{module}.note.set"))
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .ui(Form::new(&format!("{module}.note.set.title")))
            .handler(move |call: Call<Note>| {
                let key = key.clone();
                async move {
                    let text = call.params().text.clone();
                    call.tx()?
                        .execute(Statement::from_sql_and_values(
                            DbBackend::Postgres,
                            "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
                             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                            [
                                DbValue::from(key),
                                DbValue::Json(Some(Box::new(json!(text)))),
                            ],
                        ))
                        .await?;
                    Ok(Note { text })
                }
            }),
    )
    .i18n(
        "en",
        &json!({ module: { "note": { "title": "Note", "text": "Text", "set": { "title": "Set the note" } } } })
            .to_string(),
    );
    Ok(())
}

fn composed(issuer: &Issuer) -> Builder {
    Harness::new()
        .product(&DNS)
        .product(&PANEL)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .license_key(issuer.key())
}

fn features(harness: &Harness) -> std::sync::Arc<dyn Features> {
    harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set")
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused_by_license(reply: &Reply) -> &keel::sdk::serde_json::Map<String, Value> {
    let error = reply.error().expect("the license refuses the call");
    assert_eq!(error.code, ErrorCode::License, "{error:?}");
    assert_eq!(reply.status(), 402);
    assert_eq!(error.key, LICENSE);
    &error.params
}

async fn lease(harness: &Harness, issuer: &Issuer, products: &[&str]) {
    let now = harness.clock().now();
    let signed = issuer
        .sign(&json!({
            "instance_id": harness.instance(),
            "issued_at": now,
            "expires_at": now + keel::sdk::chrono::Days::new(365),
            "grace_days": 30,
            "products": products
                .iter()
                .map(|id| json!({ "id": id, "edition": "lite" }))
                .collect::<Vec<_>>(),
        }))
        .expect("the lease is signed");
    let installed = harness
        .call(Who::Admin, INSTALL, json!({ "lease": signed }))
        .await;
    let listed = data(&installed)["products"].clone();
    let features = features(harness);
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while products.iter().any(|id| !features.may_enable(id))
        || ["dns", "panel"]
            .iter()
            .any(|id| !products.contains(id) && features.may_enable(id))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease {listed} did not settle"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn note(harness: &Harness, module: &str, text: &str) -> Reply {
    harness
        .call(
            Who::A1,
            &format!("{module}.note.set"),
            json!({ "text": text }),
        )
        .await
}

async fn edit_disabled(harness: &Harness, module: &str) -> Value {
    let http = harness.http().await;
    let descriptor = http
        .get(Who::A1, &format!("/api/v1/functions/{module}.note.get"))
        .await;
    assert_eq!(descriptor.status, 200, "{}", descriptor.body);
    let disabled = descriptor.json()["ui"]["actions"][0]["disabled"].clone();
    http.stop().await.expect("the server stops");
    disabled
}

const AUDITED: &str = "SELECT function || ':' || coalesce(error_code, '') FROM audit_call WHERE function = 'license.install'";

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

async fn license(socket: &std::path::Path) -> Value {
    let mut client = Client::connect(socket)
        .await
        .expect("the socket accepts the owner");
    match client
        .call(GET, json!({}), CallOptions::default())
        .await
        .expect("the socket answers")
    {
        Envelope::Ok { data, .. } => data,
        other => panic!("license.get answers {other:?}"),
    }
}

#[tokio::test]
async fn keel_license_install_sends_the_file_over_the_socket_and_the_compiled_key_refuses_a_foreign_one()
 {
    let site = site().await;
    let runtime = Runtime::new()
        .module(&keel_system::audit::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE);
    let running = runtime.start(&site.config).await.expect("keel starts");
    let instance = std::fs::read_to_string(site.config.data_dir.join("instance.id"))
        .expect("the instance id")
        .trim()
        .to_owned();
    let before = license(running.socket()).await;
    assert_eq!(before["state"], json!("none"));
    assert_eq!(before["instanceId"], json!(instance));

    let issuer = Issuer::generate().expect("a throwaway license key");
    let lease = site.dir.path().join("license.lease");
    std::fs::write(
        &lease,
        issuer
            .sign(&json!({
                "instance_id": instance,
                "issued_at": "2026-09-25T00:00:00Z",
                "expires_at": "2027-09-25T00:00:00Z",
                "grace_days": 30,
                "products": [],
            }))
            .expect("the lease is signed"),
    )
    .expect("the lease file is written");
    let args = vec![
        "keel".to_owned(),
        "--config".to_owned(),
        site.config_path.display().to_string(),
        "license".to_owned(),
        "install".to_owned(),
        lease.display().to_string(),
    ];
    let cli = runtime.clone();
    let exit = tokio::task::spawn_blocking(move || cli.run_from(args))
        .await
        .expect("the command finishes");
    assert_eq!(exit, ExitCode::FAILURE);
    assert_eq!(license(running.socket()).await, before);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while site.database.rows(AUDITED).await != ["license.install:validation"] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the refused install is not audited"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let missing = vec![
        "keel".to_owned(),
        "license".to_owned(),
        "install".to_owned(),
        site.dir.path().join("missing.lease").display().to_string(),
        "--socket".to_owned(),
        site.config.socket.display().to_string(),
    ];
    let exit = tokio::task::spawn_blocking(move || runtime.run_from(missing))
        .await
        .expect("the command finishes");
    assert_eq!(exit, ExitCode::FAILURE);

    running.stop().await.expect("keel stops");
    drop(site.dir);
    site.database.finish().await;
}

#[tokio::test]
async fn a_lease_that_lists_dns_alone_refuses_keel_product_enable_panel() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = composed(&issuer).enabled(["dns"]).boot().await;
    assert!(features(&harness).may_enable("panel"));
    lease(&harness, &issuer, &["dns"]).await;

    let refused = harness
        .call(Who::Admin, ENABLE, json!({ "id": "panel" }))
        .await;
    assert_eq!(refused_by_license(&refused)["product"], json!("panel"));
    let dns = harness
        .call(Who::Admin, ENABLE, json!({ "id": "dns" }))
        .await;
    assert_eq!(data(&dns)["state"], json!("enabled"));
    let listed = harness
        .call(Who::Admin, "keel.product.list", json!({}))
        .await;
    let states: Vec<(Value, Value)> = data(&listed)["data"]
        .as_array()
        .expect("product rows")
        .iter()
        .map(|row| (row["id"].clone(), row["state"].clone()))
        .collect();
    assert_eq!(
        states,
        [
            (json!("dns"), json!("enabled")),
            (json!("panel"), json!("disabled"))
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn an_enabled_product_the_new_lease_drops_turns_read_only_with_a_problem() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = composed(&issuer).boot().await;
    lease(&harness, &issuer, &["dns", "panel"]).await;
    data(&note(&harness, "sites", "kept").await);
    data(&note(&harness, "zones", "kept").await);
    assert_eq!(edit_disabled(&harness, "sites").await, Value::Null);

    lease(&harness, &issuer, &["dns"]).await;
    assert_eq!(
        features(&harness).degradation("sites"),
        Some(Degradation::Unlisted {
            product: "panel".to_owned()
        })
    );
    assert_eq!(features(&harness).degradation("zones"), None);
    let refused = note(&harness, "sites", "dropped").await;
    let params = refused_by_license(&refused);
    assert_eq!(params["product"], json!("panel"));
    assert_eq!(params["function"], json!("sites.note.set"));
    let kept = harness.call(Who::A1, "sites.note.get", json!({})).await;
    assert_eq!(data(&kept)["text"], json!("kept"));
    data(&note(&harness, "zones", "still").await);
    assert_eq!(edit_disabled(&harness, "sites").await, json!(LICENSE));
    assert_eq!(edit_disabled(&harness, "zones").await, Value::Null);

    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !scheduler.is_leader() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the scheduler did not lead"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let run = scheduler.run_now(POLL_JOB).await.expect("the poll starts");
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(run.run))
        .await
        .expect("the poll ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok);
    let open = harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || severity FROM notify_problem WHERE cleared_at IS NULL")
        .await;
    assert_eq!(open, [format!("{PRODUCT_UNLISTED} panel warning")]);
    let license = harness.call(Who::Admin, GET, json!({})).await;
    assert_eq!(
        data(&license)["products"],
        json!([{ "id": "dns", "edition": "lite" }])
    );
    harness.finish().await;
}
