use std::sync::Mutex;

use keel::prelude::*;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel::types::ErrorCode;
use keel::{ExitCode, Runtime};
use keel_testkit::{Builder, Harness, TestDatabase, Who, tokio};

use crate::support::{Site, composed, create, data, eventually, rows, state_becomes};

const CREATE: &str = "hosting.site.create";
const ZONE_CREATE: &str = "dns.zone.create";

static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    name: String,
    function: &'static str,
    transaction: String,
    principal: Id,
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.0.1",
    sdk: VERSION,
    requires: &["hosting_site", "dns"],
    migrations: None,
    init: probe,
};

fn probe(r: &mut Registrar) -> Result<()> {
    r.within(CREATE, |call| seen(call, CREATE, "domain"))
        .within(ZONE_CREATE, |call| seen(call, ZONE_CREATE, "name"));
    Ok(())
}

async fn seen(call: Call<Value>, function: &'static str, field: &str) -> Result<()> {
    let row = call
        .tx()?
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT txid_current()::text",
        ))
        .await?
        .ok_or_else(|| KeelError::internal("probe.error.no_transaction"))?;
    let transaction: String = row.try_get_by_index(0)?;
    SEEN.lock().expect("the probe").push(Seen {
        name: call.params()[field].as_str().unwrap_or_default().to_owned(),
        function,
        transaction,
        principal: call.principal().id,
    });
    Ok(())
}

fn seen_for(name: &str) -> Vec<Seen> {
    SEEN.lock()
        .expect("the probe")
        .iter()
        .filter(|seen| seen.name == name)
        .cloned()
        .collect()
}

fn with_dns(site: &Site) -> Builder {
    composed(site)
        .product(&keel_dns::MANIFEST)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::stats::MODULE)
        .section(
            "dns",
            json!({
                "zones_dir": site.path("zones").display().to_string(),
                "include_dir": site.path("named").display().to_string(),
                "nameserver": "ns1.example.net",
                "hostmaster": "hostmaster.example.net",
            }),
        )
}

async fn user(harness: &Harness, who: Who, name: &str) -> String {
    let id = create(harness, who, name).await;
    state_becomes(harness, &id, "active").await;
    id
}

async fn one(harness: &Harness, sql: &str) -> String {
    let found = rows(harness, sql).await;
    assert_eq!(found.len(), 1, "{sql}: {found:?}");
    found[0].clone()
}

#[tokio::test]
async fn a_site_with_a_zone_commits_both_in_one_transaction_under_the_same_account() {
    let site = Site::new();
    let harness = with_dns(&site).module(&PROBE).boot().await;
    assert!(harness.modules().contains(&"dns_local".to_owned()));
    assert!(!harness.modules().contains(&"dns_peer".to_owned()));
    let a1 = harness.fixtures().accounts.a1.to_string();
    let alice = user(&harness, Who::A1, "alice").await;

    for (who, domain) in [(Who::A1, "own.example"), (Who::Admin, "on-behalf.example")] {
        let reply = harness
            .call(
                who,
                CREATE,
                json!({ "user": alice, "domain": domain, "dnsZone": true }),
            )
            .await;
        let id = data(&reply)["id"].as_str().expect("an id").to_owned();
        assert_eq!(
            one(
                &harness,
                &format!("SELECT account_id FROM hosting_site WHERE id = '{id}'")
            )
            .await,
            a1
        );
        assert_eq!(
            one(
                &harness,
                &format!("SELECT account_id FROM dns_zone WHERE name = '{domain}'")
            )
            .await,
            a1
        );
        let seen = seen_for(domain);
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0].function, CREATE);
        assert_eq!(seen[1].function, ZONE_CREATE);
        assert_eq!(seen[0].transaction, seen[1].transaction, "{seen:?}");
        assert_eq!(seen[0].principal, seen[1].principal, "{seen:?}");
        assert_eq!(seen[0].principal, harness.principal(who).id);
        let sql = format!("SELECT state FROM dns_zone WHERE name = '{domain}'");
        eventually(&format!("the zone {domain} applied"), async || {
            rows(&harness, &sql).await == ["active"]
        })
        .await;
    }
    harness.finish().await;
}

#[tokio::test]
async fn a_dns_validation_error_reaches_the_hosting_form_as_a_field_key() {
    let site = Site::new();
    let harness = with_dns(&site).boot().await;
    let alice = user(&harness, Who::A1, "alice").await;
    let taken = harness
        .call(Who::A2, ZONE_CREATE, json!({ "name": "taken.example" }))
        .await;
    data(&taken);

    let refused = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "taken.example", "dnsZone": true }),
        )
        .await;
    let error = refused.error().expect("the zone name is taken");
    assert_eq!(error.code, ErrorCode::Validation, "{error:?}");
    assert_eq!(error.fields.len(), 1, "{error:?}");
    assert_eq!(error.fields[0].path, "domain");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("dns.zone.error.exists")
    );
    assert_eq!(error.fields[0].params["name"], json!("taken.example"));
    assert!(
        rows(&harness, "SELECT id FROM hosting_site")
            .await
            .is_empty()
    );
    assert_eq!(
        rows(&harness, "SELECT account_id FROM dns_zone").await,
        [harness.fixtures().accounts.a2.to_string()]
    );
    assert!(!site.vhost("taken.example").exists());
    harness.finish().await;
}

#[tokio::test]
async fn without_dns_the_bridge_falls_to_dns_peer_which_answers_unavailable() {
    let site = Site::new();
    let harness = composed(&site)
        .product(&keel_dns::MANIFEST)
        .enabled(["panel"])
        .boot()
        .await;
    let modules = harness.modules();
    assert!(modules.contains(&"dns_peer".to_owned()), "{modules:?}");
    assert!(!modules.contains(&"dns_local".to_owned()), "{modules:?}");
    assert!(!modules.contains(&"dns".to_owned()), "{modules:?}");
    let alice = user(&harness, Who::A1, "alice").await;

    let refused = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "zone.example", "dnsZone": true }),
        )
        .await;
    let error = refused.error().expect("no neighbour is paired");
    assert_eq!(error.code, ErrorCode::Unavailable, "{error:?}");
    assert_eq!(error.key, "dns_peer.error.unpaired");
    assert_eq!(error.params["zone"], json!("zone.example"));
    assert!(
        rows(&harness, "SELECT id FROM hosting_site")
            .await
            .is_empty()
    );

    let plain = harness
        .call(
            Who::A1,
            CREATE,
            json!({ "user": alice, "domain": "zone.example" }),
        )
        .await;
    data(&plain);
    harness.finish().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disabling_dns_under_a_local_bridge_needs_switch_remote() {
    let site = Site::new();
    let harness = with_dns(&site).boot().await;
    harness
        .db()
        .execute_unprepared(
            "INSERT INTO keel_kv (key, value) VALUES ('products', '{\"dns\": {\"enabledAt\": \"2026-09-26T00:00:00Z\", \"by\": \"install\"}, \"panel\": {\"enabledAt\": \"2026-09-26T00:00:00Z\", \"by\": \"install\"}}') ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        )
        .await
        .expect("the enabled set is stored");
    let refused = harness
        .call(Who::Admin, "keel.product.disable", json!({ "id": "dns" }))
        .await;
    let error = refused.error().expect("panel reaches dns locally");
    assert_eq!(error.code, ErrorCode::Conflict, "{error:?}");
    assert_eq!(error.key, "keel.product.error.bridged");
    assert_eq!(error.params["by"], json!("panel"));
    harness.finish().await;

    let database = TestDatabase::create().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let config = dir.path().join("config.yaml");
    std::fs::write(
        &config,
        format!(
            "listen: 127.0.0.1:0\nsocket: {}\ndata_dir: {}\ndatabase: {}\nlog: {{ level: warn, format: text }}\n",
            dir.path().join("run").join("core.sock").display(),
            dir.path().join("lib").display(),
            database.config().database.expose(),
        ),
    )
    .expect("the config file");
    let cli = async |rest: &[&str]| {
        let mut args = vec![
            "keel".to_owned(),
            "--config".to_owned(),
            config.display().to_string(),
            "product".to_owned(),
        ];
        args.extend(rest.iter().map(|arg| (*arg).to_owned()));
        tokio::task::spawn_blocking(move || {
            Runtime::new()
                .product(&keel_dns::MANIFEST)
                .product(&keel_panel::MANIFEST)
                .run_from(args)
        })
        .await
        .expect("the command finishes")
    };
    let enabled = async || {
        database
            .rows("SELECT jsonb_object_keys(value) FROM keel_kv WHERE key = 'products' ORDER BY 1")
            .await
    };

    assert_eq!(cli(&["enable", "dns"]).await, ExitCode::SUCCESS);
    assert_eq!(cli(&["enable", "panel"]).await, ExitCode::SUCCESS);
    assert_eq!(enabled().await, ["dns", "panel"]);
    assert_eq!(cli(&["disable", "dns"]).await, ExitCode::FAILURE);
    assert_eq!(enabled().await, ["dns", "panel"]);
    assert_eq!(
        cli(&["disable", "dns", "--switch-remote"]).await,
        ExitCode::SUCCESS
    );
    assert_eq!(enabled().await, ["panel"]);
    drop(dir);
    database.finish().await;
}
