use std::fs;
use std::sync::{Mutex, PoisonError};

use keel::prelude::*;
use keel::sdk::chrono::Duration;
use keel::sdk::{InstanceInfoSource, Registrar};
use keel::types::ErrorCode;
use keel_system::stats::COLLECT_JOB;
use keel_testkit::{CallOptions, Harness, Reply, Who, tokio};

use crate::support::{Site, composed, data, eventually, rows, run_job, state_becomes};

const KIND: &str = "hosting.sites";
const ORIGIN: &str = "bill";

static RESUMED: Mutex<Vec<Id>> = Mutex::new(Vec::new());

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &["hosting_site"],
    migrations: None,
    init: probe,
};

fn probe(r: &mut Registrar) -> Result<()> {
    r.contribute(
        AccountLifecycle::new().on_resume(|call: Call<Id>| async move {
            RESUMED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*call.params());
            Ok(())
        }),
    );
    Ok(())
}

async fn boot(site: &Site) -> Harness {
    composed(site)
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::svc::MODULE)
        .module(&PROBE)
        .clock("2026-10-04T12:00:00Z".parse().expect("an instant"))
        .boot()
        .await
}

async fn call(harness: &Harness, verb: &str, params: Value) -> Reply {
    harness
        .call_with(
            harness.principal(Who::Admin),
            &format!("svc.{KIND}.{verb}"),
            params,
            CallOptions {
                idempotency_key: Some(format!("{verb}-{}", Id::generate())),
                ..CallOptions::default()
            },
        )
        .await
}

async fn open(harness: &Harness, external_id: &str, site_limit: Value) -> Value {
    let reply = call(
        harness,
        "open",
        json!({
            "externalId": external_id,
            "account": { "origin": ORIGIN, "externalId": "acc-7" },
            "params": { "siteLimit": site_limit },
        }),
    )
    .await;
    data(&reply).clone()
}

async fn act(harness: &Harness, verb: &str, external_id: &str) -> Value {
    let reply = call(harness, verb, json!({ "externalId": external_id })).await;
    data(&reply).clone()
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("a string was expected, got {value}"))
        .to_owned()
}

async fn user_in(harness: &Harness, account: &str, name: &str) -> String {
    let reply = harness
        .call(
            Who::Admin,
            "hosting.user.create",
            json!({ "name": name, "account": account }),
        )
        .await;
    let id = text(&data(&reply)["id"]);
    state_becomes(harness, &id, "active").await;
    id
}

async fn site_for(harness: &Harness, user: &str, domain: &str) -> Reply {
    harness
        .call(
            Who::Admin,
            "hosting.site.create",
            json!({ "user": user, "domain": domain }),
        )
        .await
}

async fn suspended(harness: &Harness, account: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT domain || ' ' || suspended FROM hosting_site WHERE account_id = '{account}' ORDER BY domain"
        ),
    )
    .await
}

fn refused(site: &Site, domain: &str) -> bool {
    fs::read_to_string(site.vhost(domain)).is_ok_and(|text| text.contains("return 403;"))
}

async fn served(site: &Site, domain: &str, wanted: bool) {
    eventually(
        &format!("the server block of {domain} refusing: {}", !wanted),
        async || site.vhost(domain).exists() && refused(site, domain) != wanted,
    )
    .await;
}

async fn resumed(account: &str) {
    let account: Id = account.parse().expect("an account id");
    eventually(
        &format!("the lifecycle of the resume of {account}"),
        async || {
            RESUMED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&account)
        },
    )
    .await;
    RESUMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|seen| *seen != account);
}

async fn switched_to(harness: &Harness, account: &str, wanted: &[&str]) {
    eventually(
        &format!("the sites of {account} switched to {wanted:?}"),
        async || suspended(harness, account).await == wanted,
    )
    .await;
}

async fn stays(harness: &Harness, account: &str, wanted: &[&str]) {
    let until = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < until {
        assert_eq!(suspended(harness, account).await, wanted);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn account_call(harness: &Harness, verb: &str, account: &str) {
    let reply = harness
        .call(
            Who::Admin,
            &format!("keel.account.{verb}"),
            json!({ "id": account }),
        )
        .await;
    assert_eq!(data(&reply)["changed"], json!(true), "{verb}");
}

async fn limit_of(harness: &Harness, account: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT value::text FROM keel_limit WHERE account_id = '{account}' AND key = 'hosting.site.max'"
        ),
    )
    .await
}

#[tokio::test]
async fn svc_hosting_sites_open_and_set_params_set_the_site_limit_of_the_account() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", json!(1)).await;
    assert_eq!(opened["status"], json!("active"));
    assert_eq!(opened["state"], json!({ "sitesUsed": 0 }));
    let account = text(&opened["accountId"]);
    assert_eq!(limit_of(&harness, &account).await, ["1"]);
    assert_eq!(
        rows(
            &harness,
            "SELECT service_id || ' ' || account_id || ' ' || held FROM hosting_site_service"
        )
        .await,
        [format!("{} {account} false", text(&opened["serviceId"]))]
    );

    let alice = user_in(&harness, &account, "alice").await;
    data(&site_for(&harness, &alice, "one.example").await);
    let over = site_for(&harness, &alice, "two.example").await;
    let error = over.error().expect("the limit is reached");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.params["max"], json!(1));

    let raised = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "siteLimit": null } }),
    )
    .await;
    assert_eq!(data(&raised)["state"], json!({ "sitesUsed": 1 }));
    assert_eq!(limit_of(&harness, &account).await, ["null"]);
    data(&site_for(&harness, &alice, "two.example").await);
    let missing = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": {} }),
    )
    .await;
    assert_eq!(
        missing.error().map(|error| error.code),
        Some(ErrorCode::Validation)
    );
    assert_eq!(limit_of(&harness, &account).await, ["null"]);
    harness.finish().await;
}

#[tokio::test]
async fn suspend_and_resume_switch_the_sites_and_an_account_resume_keeps_a_suspended_service_off() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", json!(5)).await;
    let account = text(&opened["accountId"]);
    let alice = user_in(&harness, &account, "alice").await;
    data(&site_for(&harness, &alice, "svc.example").await);
    let carol = crate::support::create(&harness, Who::A1, "carol").await;
    state_becomes(&harness, &carol, "active").await;
    data(&site_for(&harness, &carol, "a1.example").await);
    served(&site, "svc.example", true).await;
    served(&site, "a1.example", true).await;

    let held = act(&harness, "suspend", "svc-1").await;
    assert_eq!(held["status"], json!("suspended"));
    switched_to(&harness, &account, &["svc.example true"]).await;
    served(&site, "svc.example", false).await;
    assert!(!refused(&site, "a1.example"));
    assert!(!site.locked("hp_alice"));
    let late = data(&site_for(&harness, &alice, "late.example").await).clone();
    assert_eq!(late["suspended"], json!(true));
    served(&site, "late.example", false).await;

    account_call(&harness, "suspend", &account).await;
    account_call(&harness, "resume", &account).await;
    resumed(&account).await;
    assert_eq!(
        suspended(&harness, &account).await,
        ["late.example true", "svc.example true"]
    );
    assert!(refused(&site, "svc.example"));

    let back = act(&harness, "resume", "svc-1").await;
    assert_eq!(back["status"], json!("active"));
    switched_to(
        &harness,
        &account,
        &["late.example false", "svc.example false"],
    )
    .await;
    served(&site, "svc.example", true).await;
    served(&site, "late.example", true).await;

    account_call(&harness, "suspend", &account).await;
    switched_to(
        &harness,
        &account,
        &["late.example true", "svc.example true"],
    )
    .await;
    act(&harness, "resume", "svc-1").await;
    stays(
        &harness,
        &account,
        &["late.example true", "svc.example true"],
    )
    .await;
    account_call(&harness, "resume", &account).await;
    resumed(&account).await;
    served(&site, "svc.example", true).await;

    let closed = act(&harness, "close", "svc-1").await;
    assert_eq!(closed["status"], json!("closed"));
    served(&site, "svc.example", false).await;
    let again = open(&harness, "svc-2", json!(5)).await;
    assert_eq!(again["accountId"], json!(account));
    assert_eq!(again["state"], json!({ "sitesUsed": 2 }));
    served(&site, "svc.example", true).await;
    served(&site, "late.example", true).await;
    harness.finish().await;
}

#[tokio::test]
async fn a_second_service_of_the_kind_keeps_the_sites_served_while_the_first_is_held() {
    let site = Site::new();
    let harness = boot(&site).await;
    let first = open(&harness, "svc-1", json!(5)).await;
    let second = open(&harness, "svc-2", json!(5)).await;
    let account = text(&first["accountId"]);
    assert_eq!(second["accountId"], json!(account));
    let alice = user_in(&harness, &account, "alice").await;
    data(&site_for(&harness, &alice, "svc.example").await);
    served(&site, "svc.example", true).await;

    assert_eq!(
        act(&harness, "suspend", "svc-1").await["status"],
        json!("suspended")
    );
    stays(&harness, &account, &["svc.example false"]).await;
    assert!(!refused(&site, "svc.example"));
    let opened_meanwhile = data(&site_for(&harness, &alice, "late.example").await).clone();
    assert_eq!(opened_meanwhile["suspended"], json!(false));

    act(&harness, "suspend", "svc-2").await;
    switched_to(
        &harness,
        &account,
        &["late.example true", "svc.example true"],
    )
    .await;
    served(&site, "svc.example", false).await;

    act(&harness, "resume", "svc-1").await;
    switched_to(
        &harness,
        &account,
        &["late.example false", "svc.example false"],
    )
    .await;
    served(&site, "svc.example", true).await;

    assert_eq!(
        act(&harness, "close", "svc-1").await["status"],
        json!("closed")
    );
    switched_to(
        &harness,
        &account,
        &["late.example true", "svc.example true"],
    )
    .await;
    served(&site, "svc.example", false).await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT state FROM keel_account WHERE id = '{account}'")
        )
        .await,
        ["active"]
    );

    act(&harness, "resume", "svc-2").await;
    switched_to(
        &harness,
        &account,
        &["late.example false", "svc.example false"],
    )
    .await;
    served(&site, "svc.example", true).await;
    served(&site, "late.example", true).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT s.external_id || ' ' || h.held FROM hosting_site_service h \
             JOIN svc_service s ON s.id = h.service_id ORDER BY s.external_id"
        )
        .await,
        ["svc-1 true", "svc-2 false"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn closing_the_newer_of_two_services_gives_back_the_site_limit_of_the_older() {
    let site = Site::new();
    let harness = boot(&site).await;
    let older = open(&harness, "svc-1", json!(10)).await;
    let account = text(&older["accountId"]);
    assert_eq!(limit_of(&harness, &account).await, ["10"]);
    open(&harness, "svc-2", json!(50)).await;
    assert_eq!(limit_of(&harness, &account).await, ["50"]);

    let changed = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "siteLimit": 1 } }),
    )
    .await;
    data(&changed);
    assert_eq!(limit_of(&harness, &account).await, ["50"]);
    assert_eq!(
        rows(
            &harness,
            "SELECT s.external_id || ' ' || h.site_limit FROM hosting_site_service h \
             JOIN svc_service s ON s.id = h.service_id ORDER BY s.external_id"
        )
        .await,
        ["svc-1 1", "svc-2 50"]
    );

    act(&harness, "suspend", "svc-2").await;
    assert_eq!(limit_of(&harness, &account).await, ["1"]);
    act(&harness, "resume", "svc-2").await;
    assert_eq!(limit_of(&harness, &account).await, ["50"]);

    assert_eq!(
        act(&harness, "close", "svc-2").await["status"],
        json!("closed")
    );
    assert_eq!(limit_of(&harness, &account).await, ["1"]);
    let alice = user_in(&harness, &account, "alice").await;
    data(&site_for(&harness, &alice, "one.example").await);
    let over = site_for(&harness, &alice, "two.example").await;
    let error = over.error().expect("the limit of the older service holds");
    assert_eq!(error.code, ErrorCode::LimitExceeded);
    assert_eq!(error.params["max"], json!(1));

    act(&harness, "close", "svc-1").await;
    assert_eq!(limit_of(&harness, &account).await, ["1"]);
    open(&harness, "svc-3", json!(null)).await;
    assert_eq!(limit_of(&harness, &account).await, ["null"]);
    harness.finish().await;
}

#[tokio::test]
async fn stat_answers_sites_used_and_login_lands_on_the_site_list() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", json!(null)).await;
    let account = text(&opened["accountId"]);
    let alice = user_in(&harness, &account, "alice").await;
    data(&site_for(&harness, &alice, "one.example").await);
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().advance(Duration::hours(1));
    data(&site_for(&harness, &alice, "two.example").await);
    run_job(&harness, COLLECT_JOB).await;

    let stat = call(
        &harness,
        "stat",
        json!({ "externalId": "svc-1", "from": "2026-10-04T12:00:00Z", "to": "2026-10-04T14:00:00Z" }),
    )
    .await;
    assert_eq!(data(&stat)["usage"], json!({ "sitesUsed": 2 }));
    let morning = call(
        &harness,
        "stat",
        json!({ "externalId": "svc-1", "from": "2026-10-04T00:00:00Z", "to": "2026-10-04T13:00:00Z" }),
    )
    .await;
    assert_eq!(data(&morning)["usage"], json!({ "sitesUsed": 1 }));

    let nobody = call(&harness, "login", json!({ "externalId": "svc-1" })).await;
    assert_eq!(
        nobody.error().map(|error| error.key.as_str()),
        Some("svc.error.no_principal")
    );
    let customer = harness
        .call(
            Who::Admin,
            "keel.principal.upsert",
            json!({
                "origin": ORIGIN,
                "externalId": "client-42",
                "login": "client-42",
                "level": "user",
                "account": { "origin": ORIGIN, "externalId": "acc-7" },
            }),
        )
        .await;
    let customer = text(&data(&customer)["id"]);
    let login = call(&harness, "login", json!({ "externalId": "svc-1" })).await;
    assert!(text(&data(&login)["url"]).starts_with("/login?key=kok_"));
    assert_eq!(
        rows(
            &harness,
            "SELECT principal_id || ' ' || redirect FROM auth_login_key"
        )
        .await,
        [format!("{customer} /f/hosting.site.list")]
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_catalogue_offers_hosting_sites_beside_dns_zones_on_an_installation_with_both() {
    let site = Site::new();
    let harness = composed(&site)
        .product(&keel_dns::MANIFEST)
        .module(&keel_system::svc::MODULE)
        .boot()
        .await;
    let kinds: Vec<&String> = harness
        .registry()
        .contributions()
        .of::<InstanceInfoSource>()
        .flat_map(|source| source.service_kinds())
        .collect();
    assert_eq!(kinds, ["dns.zones", KIND]);
    let catalogue = harness.call(Who::Admin, "svc.kinds", json!({})).await;
    let entries = data(&catalogue)["data"].as_array().expect("the entries");
    let offered: Vec<(&str, &str)> = entries
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().expect("a kind"),
                entry["title"].as_str().expect("a title"),
            )
        })
        .collect();
    assert_eq!(
        offered,
        [
            ("dns.zones", "dns.service.zones.title"),
            (KIND, "hosting.site.service.title"),
        ]
    );
    let sites = &entries[1];
    assert_eq!(sites["params"]["required"], json!(["siteLimit"]));
    assert_eq!(
        sites["params"]["properties"]["siteLimit"]["type"],
        json!(["integer", "null"])
    );
    assert_eq!(
        sites["state"]["properties"]["sitesUsed"]["type"],
        json!("integer")
    );
    assert_eq!(sites["functions"]["stat"], json!("svc.hosting.sites.stat"));
    harness.finish().await;
}
