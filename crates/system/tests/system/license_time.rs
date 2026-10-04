use std::time::Duration;

use keel_sdk::chrono::{DateTime, Days, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use keel_sdk::serde_json::Map;
use keel_sdk::system::{Client, Features};
use keel_sdk::types::LicenseState;
use keel_system::cron::ERROR_RUNNING;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::license::{CLOCK_JOB, EXPIRED, EXPIRING, GET, INSTALL, KV_CLOCK, MODULE};
use keel_testkit::license::Issuer;
use keel_testkit::{Builder, CallOptions, ErrorCode, Harness, Reply, Who, tokio};

const NOW: &str = "2026-09-25T00:00:00Z";
const PATIENCE: Duration = Duration::from_secs(10);
const NOTE_GET: &str = "probe.note.get";
const NOTE_SET: &str = "probe.note.set";
const NOTE_KEY: &str = "probe_note";
const PASSWORD: &str = "correct-horse";
const POLL_JOB: &str = "notify.problems.poll";
const LICENSE: &str = "keel.error.license";

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

static ORBIT: ProductManifest = ProductManifest {
    id: "orbit",
    name: "product.orbit.brand",
    version: "0.1.0",
    api: keel_sdk::types::DESCRIPTOR_VERSION,
    brand: Brand {
        logo: "product.orbit.logo",
        hue: 200,
        chroma: 0.1,
    },
    modules: &[&PROBE],
    bridges: &[],
    menu: &[],
    levels: LevelLabels {
        labels: &[],
        disabled: &[],
    },
    account_kinds: &["user"],
    editions: &[Edition {
        id: "basic",
        features: &[],
    }],
    i18n: &[("en", r#"{ "product": { "orbit": { "brand": "Orbit" } } }"#)],
    assets: &[],
};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Note {
    text: String,
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Note>(NOTE_GET)
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .ui(ObjectScreen::new("probe.note.title")
                .action(Action::new("edit", ActionRole::Edit).screen(NOTE_SET))
                .action(Action::new("refresh", ActionRole::Refresh)))
            .handler(|call: Call<Nothing>| async move {
                let row = call
                    .db()
                    .query_one(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "SELECT value #>> '{}' AS text FROM keel_kv WHERE key = $1",
                        [DbValue::from(NOTE_KEY)],
                    ))
                    .await?;
                let text = match row {
                    Some(row) => row.try_get::<String>("", "text")?,
                    None => String::new(),
                };
                Ok(Note { text })
            }),
    )
    .function(
        function::<Note, Note>(NOTE_SET)
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .ui(Form::new("probe.note.set.title"))
            .handler(|call: Call<Note>| async move {
                let text = call.params().text.clone();
                call.tx()?
                    .execute(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                        [
                            DbValue::from(NOTE_KEY),
                            DbValue::Json(Some(Box::new(json!(text)))),
                        ],
                    ))
                    .await?;
                Ok(Note { text })
            }),
    )
    .i18n(
        "en",
        r#"{ "probe": { "note": { "title": "Note", "text": "Text", "set": { "title": "Set the note" } } } }"#,
    );
    Ok(())
}

fn now() -> DateTime<Utc> {
    NOW.parse().expect("an instant")
}

fn expires() -> DateTime<Utc> {
    now() + Days::new(365)
}

fn grace_ends() -> DateTime<Utc> {
    expires() + Days::new(30)
}

fn composed(issuer: &Issuer) -> Builder {
    Harness::new()
        .product(&ORBIT)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .license_key(issuer.key())
        .clock(now())
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused_by_license<'a>(reply: &'a Reply, function: &str) -> &'a Map<String, Value> {
    let error = reply.error().expect("the license refuses the call");
    assert_eq!(error.code, ErrorCode::License, "{error:?}");
    assert_eq!(reply.status(), 402);
    assert_eq!(error.key, LICENSE);
    assert_eq!(error.params["function"], json!(function));
    &error.params
}

async fn install(harness: &Harness, issuer: &Issuer, expires_at: DateTime<Utc>) {
    let signed = issuer
        .sign(&json!({
            "instance_id": harness.instance(),
            "issued_at": now(),
            "expires_at": expires_at,
            "grace_days": 30,
            "products": [{ "id": "orbit", "edition": "basic" }],
        }))
        .expect("the lease is signed");
    data(
        &harness
            .call(Who::Admin, INSTALL, json!({ "lease": signed }))
            .await,
    );
    let features = harness
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while features.license().and_then(|license| license.expires_at) != Some(expires_at) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lease did not settle within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn state(harness: &Harness) -> Value {
    data(&harness.call(Who::Admin, GET, json!({})).await)["state"].clone()
}

async fn write(harness: &Harness, text: &str) -> Reply {
    harness
        .call(Who::A1, NOTE_SET, json!({ "text": text }))
        .await
}

async fn read(harness: &Harness) -> Value {
    data(&harness.call(Who::A1, NOTE_GET, json!({})).await)["text"].clone()
}

async fn edit_action(harness: &Harness) -> Value {
    let http = harness.http().await;
    let descriptor = http
        .get(Who::A1, &format!("/api/v1/functions/{NOTE_GET}"))
        .await;
    assert_eq!(descriptor.status, 200, "{}", descriptor.body);
    let actions = descriptor.json()["ui"]["actions"].clone();
    http.stop().await.expect("the server stops");
    actions
        .as_array()
        .expect("actions")
        .iter()
        .find(|action| action["id"] == json!("edit"))
        .cloned()
        .expect("the edit action stays in the descriptor")
}

async fn run(harness: &Harness, job: &str) {
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
    let started = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{job} did not start in time"
        );
        match scheduler.run_now(job).await {
            Ok(started) => break started,
            Err(error) if error.key() == ERROR_RUNNING => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("{job} did not start: {error:?}"),
        }
    };
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the run ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok, "{job}");
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || severity FROM notify_problem WHERE cleared_at IS NULL ORDER BY key, subject")
        .await
}

async fn clock_row(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT ((value #>> '{{}}')::timestamptz = '{}'::timestamptz)::text FROM keel_kv WHERE key = '{KV_CLOCK}'",
            harness.clock().now().to_rfc3339()
        ))
        .await
}

#[tokio::test]
async fn the_installation_works_until_expires_then_in_grace_then_read_only() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = composed(&issuer).boot().await;
    install(&harness, &issuer, expires()).await;
    assert_eq!(state(&harness).await, json!("valid"));
    data(&write(&harness, "valid").await);
    assert!(edit_action(&harness).await.get("disabled").is_none());

    harness.clock().set(expires() + Days::new(1));
    assert_eq!(state(&harness).await, json!("grace"));
    data(&write(&harness, "grace").await);
    assert!(edit_action(&harness).await.get("disabled").is_none());
    run(&harness, POLL_JOB).await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{EXPIRING} lease warning")]
    );

    harness.clock().set(grace_ends() + Days::new(1));
    assert_eq!(state(&harness).await, json!("read_only"));
    let refused = write(&harness, "read-only").await;
    let params = refused_by_license(&refused, NOTE_SET);
    assert_eq!(params["state"], json!("read_only"));
    assert_eq!(read(&harness).await, json!("grace"));
    assert_eq!(edit_action(&harness).await["disabled"], json!(LICENSE));
    run(&harness, POLL_JOB).await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{EXPIRED} lease critical")]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_clock_moved_backwards_does_not_revive_an_expired_lease() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let instance = Id::generate();
    let harness = composed(&issuer).instance(instance).boot().await;
    assert_eq!(clock_row(&harness).await, ["true"]);
    install(&harness, &issuer, expires()).await;

    harness.clock().set(grace_ends() + Days::new(1));
    assert_eq!(state(&harness).await, json!("read_only"));
    run(&harness, CLOCK_JOB).await;
    assert_eq!(clock_row(&harness).await, ["true"]);

    harness.clock().set(now());
    assert_eq!(state(&harness).await, json!("read_only"));
    refused_by_license(&write(&harness, "rewound").await, NOTE_SET);

    let database = harness.stop().await;
    let restarted = composed(&issuer)
        .instance(instance)
        .database(database)
        .boot()
        .await;
    assert_eq!(restarted.clock().now(), now());
    assert_eq!(state(&restarted).await, json!("read_only"));
    refused_by_license(&write(&restarted, "restarted").await, NOTE_SET);
    let features = restarted
        .registry()
        .services()
        .get::<dyn Features>()
        .expect("a feature set");
    assert_eq!(
        features.license().map(|license| license.state),
        Some(LicenseState::ReadOnly)
    );
    restarted.finish().await;
}

#[tokio::test]
async fn read_only_keeps_reads_login_and_license_install() {
    let issuer = Issuer::generate().expect("a throwaway license key");
    let harness = composed(&issuer)
        .module(&keel_system::auth::MODULE)
        .boot()
        .await;
    let a1 = harness.principal(Who::A1).id;
    data(
        &harness
            .call(
                Who::Owner,
                "auth.password.set",
                json!({ "principal": a1, "password": PASSWORD }),
            )
            .await,
    );
    install(&harness, &issuer, expires()).await;
    data(&write(&harness, "kept").await);

    harness.clock().set(grace_ends() + Days::new(1));
    assert_eq!(state(&harness).await, json!("read_only"));
    assert_eq!(read(&harness).await, json!("kept"));
    let login = harness
        .call_with(
            harness.principal(Who::Guest),
            "auth.login",
            json!({ "login": "a1", "password": PASSWORD }),
            CallOptions {
                client: Some(Client {
                    ip: Some("10.0.0.1".parse().expect("an address")),
                    user_agent: Some("keel-system-tests/1".into()),
                    session: None,
                }),
                ..CallOptions::default()
            },
        )
        .await;
    data(&login);
    let password = harness
        .call(
            Who::Owner,
            "auth.password.set",
            json!({ "principal": a1, "password": "another-horse" }),
        )
        .await;
    refused_by_license(&password, "auth.password.set");
    refused_by_license(&write(&harness, "refused").await, NOTE_SET);

    install(&harness, &issuer, grace_ends() + Days::new(365)).await;
    assert_eq!(state(&harness).await, json!("valid"));
    data(&write(&harness, "renewed").await);
    assert_eq!(read(&harness).await, json!("renewed"));
    harness.finish().await;
}
