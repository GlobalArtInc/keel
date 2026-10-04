use std::sync::Mutex;

use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Duration, Utc};
use keel::sdk::{InstanceInfoSource, Registrar};
use keel::types::ErrorCode;
use keel_system::stats::{AGGREGATE_JOB, COLLECT_JOB};
use keel_testkit::{CallOptions, Harness, Reply, Who, tokio};

use crate::support::{Site, composed, data, eventually, rows, run_job, state_becomes};

const KIND: &str = "dns.zones";
const ORIGIN: &str = "bill";
const NOW: &str = "2026-10-04T12:00:00Z";
const WEB: &str = "00000000000000000000000001";
const EMPTY: &str = "00000000000000000000000002";

static RESUMED: Mutex<Vec<Id>> = Mutex::new(Vec::new());

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &["dns"],
    migrations: None,
    init: probe,
};

fn probe(r: &mut Registrar) -> Result<()> {
    r.contribute(
        AccountLifecycle::new().on_resume(|call: Call<Id>| async move {
            RESUMED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(*call.params());
            Ok(())
        }),
    );
    Ok(())
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

async fn boot(site: &Site) -> Harness {
    composed(site)
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::svc::MODULE)
        .module(&PROBE)
        .clock(at(NOW))
        .boot()
        .await
}

fn func(verb: &str) -> String {
    format!("svc.{KIND}.{verb}")
}

async fn call(harness: &Harness, verb: &str, params: Value, key: Option<&str>) -> Reply {
    harness
        .call_with(
            harness.principal(Who::Admin),
            &func(verb),
            params,
            CallOptions {
                idempotency_key: key.map(str::to_owned),
                ..CallOptions::default()
            },
        )
        .await
}

fn opening(external_id: &str, account: &str, params: Value) -> Value {
    json!({
        "externalId": external_id,
        "account": { "origin": ORIGIN, "externalId": account },
        "params": params,
    })
}

async fn open(harness: &Harness, external_id: &str, account: &str, params: Value) -> Value {
    let key = format!("open-{external_id}");
    let reply = call(
        harness,
        "open",
        opening(external_id, account, params),
        Some(&key),
    )
    .await;
    data(&reply).clone()
}

async fn act(harness: &Harness, verb: &str, external_id: &str) -> Value {
    let key = format!("{verb}-{external_id}-{}", Id::generate());
    let reply = call(
        harness,
        verb,
        json!({ "externalId": external_id }),
        Some(&key),
    )
    .await;
    data(&reply).clone()
}

fn text(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("a string was expected, got {value}"))
        .to_owned()
}

fn refused(reply: &Reply, code: ErrorCode, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(error.key, key, "{error:?}");
}

async fn zone_in(harness: &Harness, account: &str, name: &str) -> Reply {
    harness
        .call(
            Who::Admin,
            "dns.zone.create",
            json!({ "name": name, "account": account }),
        )
        .await
}

async fn suspended(harness: &Harness, account: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT name || ' ' || suspended FROM dns_zone WHERE account_id = '{account}' ORDER BY name"
        ),
    )
    .await
}

async fn limit_of(harness: &Harness, account: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT value::text FROM keel_limit WHERE account_id = '{account}' AND key = '{KIND}'"
        ),
    )
    .await
}

fn included(site: &Site, zone: &str) -> bool {
    site.index()
        .contains(&format!("include \"{}\";", site.include(zone).display()))
}

async fn served(site: &Site, zone: &str, wanted: bool) {
    eventually(
        &format!("the index with {zone} served: {wanted}"),
        async || included(site, zone) == wanted,
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
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&account)
        },
    )
    .await;
    RESUMED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|seen| *seen != account);
}

async fn switched_to(harness: &Harness, account: &str, wanted: &[&str]) {
    eventually(
        &format!("the zones of {account} switched to {wanted:?}"),
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

#[tokio::test]
async fn svc_dns_zones_open_twice_with_one_key_produces_one_service() {
    let site = Site::new();
    let harness = boot(&site).await;
    let params = json!({ "zoneLimit": 50, "template": WEB });

    let first = call(
        &harness,
        "open",
        opening("bill-svc-9912", "acc-7", params.clone()),
        Some("bill-9912-open"),
    )
    .await;
    let answer = data(&first).clone();
    assert!(!first.replay);
    assert_eq!(answer["status"], json!("active"));
    assert_eq!(answer["state"], json!({ "zonesUsed": 0 }));
    let again = call(
        &harness,
        "open",
        opening("bill-svc-9912", "acc-7", params.clone()),
        Some("bill-9912-open"),
    )
    .await;
    assert!(again.replay, "the repeat is replayed from the store");
    assert_eq!(data(&again), &answer);

    let account = text(&answer["accountId"]);
    assert_eq!(
        rows(
            &harness,
            "SELECT kind || ' ' || external_id || ' ' || status || ' ' || account_id FROM svc_service"
        )
        .await,
        [format!("{KIND} bill-svc-9912 active {account}")]
    );
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT id FROM keel_account WHERE origin = '{ORIGIN}' AND external_id = 'acc-7'"
            )
        )
        .await,
        std::slice::from_ref(&account)
    );
    assert_eq!(limit_of(&harness, &account).await, ["50"]);
    assert_eq!(
        rows(
            &harness,
            "SELECT account_id || ' ' || template FROM dns_account"
        )
        .await,
        [format!("{account} {WEB}")]
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT service_id || ' ' || account_id || ' ' || held FROM dns_service"
        )
        .await,
        [format!("{} {account} false", text(&answer["serviceId"]))]
    );

    let fresh_key = call(
        &harness,
        "open",
        opening("bill-svc-9912", "acc-7", params),
        Some("bill-9912-again"),
    )
    .await;
    refused(&fresh_key, ErrorCode::Conflict, "svc.error.exists");
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM svc_service").await,
        ["1"]
    );
    let missing_key = call(
        &harness,
        "open",
        opening("bill-svc-9913", "acc-7", json!({ "zoneLimit": 5 })),
        None,
    )
    .await;
    refused(
        &missing_key,
        ErrorCode::Validation,
        "keel.error.idempotency_required",
    );
    harness.finish().await;
}

#[tokio::test]
async fn set_params_changes_the_zone_limit() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 1 })).await;
    let account = text(&opened["accountId"]);
    assert_eq!(limit_of(&harness, &account).await, ["1"]);

    data(&zone_in(&harness, &account, "one.example").await);
    let over = zone_in(&harness, &account, "two.example").await;
    refused(&over, ErrorCode::LimitExceeded, "keel.error.limit_exceeded");
    assert_eq!(over.error().unwrap().params["max"], json!(1));

    let raised = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": 3 } }),
        None,
    )
    .await;
    assert_eq!(data(&raised)["state"], json!({ "zonesUsed": 1 }));
    assert_eq!(limit_of(&harness, &account).await, ["3"]);
    data(&zone_in(&harness, &account, "two.example").await);
    data(&zone_in(&harness, &account, "three.example").await);
    refused(
        &zone_in(&harness, &account, "four.example").await,
        ErrorCode::LimitExceeded,
        "keel.error.limit_exceeded",
    );

    let unlimited = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": null } }),
        None,
    )
    .await;
    assert_eq!(data(&unlimited)["state"], json!({ "zonesUsed": 3 }));
    assert_eq!(limit_of(&harness, &account).await, ["null"]);
    data(&zone_in(&harness, &account, "four.example").await);

    let closed = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": 0 } }),
        None,
    )
    .await;
    assert_eq!(data(&closed)["state"], json!({ "zonesUsed": 4 }));
    refused(
        &zone_in(&harness, &account, "five.example").await,
        ErrorCode::LimitExceeded,
        "keel.error.limit_exceeded",
    );
    assert_eq!(
        rows(
            &harness,
            "SELECT params_json::text FROM svc_service WHERE external_id = 'svc-1'"
        )
        .await,
        [r#"{"zoneLimit": 0}"#]
    );
    assert_eq!(
        limit_of(&harness, &harness.fixtures().accounts.a1.to_string()).await,
        Vec::<String>::new()
    );

    let missing = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": {} }),
        None,
    )
    .await;
    refused(&missing, ErrorCode::Validation, "keel.error.validation");
    let negative = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": -1 } }),
        None,
    )
    .await;
    refused(&negative, ErrorCode::Validation, "keel.error.validation");
    assert_eq!(limit_of(&harness, &account).await, ["0"]);
    harness.finish().await;
}

#[tokio::test]
async fn stat_answers_zones_used() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": null })).await;
    let account = text(&opened["accountId"]);
    let a1 = harness.fixtures().accounts.a1.to_string();
    for name in ["a1-one.example", "a1-two.example", "a1-three.example"] {
        data(&zone_in(&harness, &a1, name).await);
        data(&zone_in(&harness, &a1, &format!("x.{name}")).await);
    }

    data(&zone_in(&harness, &account, "one.example").await);
    data(&zone_in(&harness, &account, "two.example").await);
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().advance(Duration::hours(1));
    let third = data(&zone_in(&harness, &account, "three.example").await)["id"].clone();
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().advance(Duration::hours(1));
    let deleted = harness
        .call(Who::Admin, "dns.zone.delete", json!({ "ids": [third] }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(1));
    run_job(&harness, COLLECT_JOB).await;

    let stat = |from: &str, to: &str| {
        let params = json!({ "externalId": "svc-1", "from": from, "to": to });
        let harness = &harness;
        async move { call(harness, "stat", params, None).await }
    };
    let whole = stat("2026-10-04T12:00:00Z", "2026-10-04T15:00:00Z").await;
    assert_eq!(data(&whole)["usage"], json!({ "zonesUsed": 3 }));
    assert_eq!(data(&whole)["serviceId"], opened["serviceId"]);
    let last = stat("2026-10-04T14:00:00Z", "2026-10-04T15:00:00Z").await;
    assert_eq!(data(&last)["usage"], json!({ "zonesUsed": 2 }));
    let first = stat("2026-10-04T12:00:00Z", "2026-10-04T13:00:00Z").await;
    assert_eq!(data(&first)["usage"], json!({ "zonesUsed": 2 }));
    let empty = stat("2026-10-05T00:00:00Z", "2026-10-06T00:00:00Z").await;
    assert_eq!(data(&empty)["usage"], json!({ "zonesUsed": 0 }));

    let closed = act(&harness, "close", "svc-1").await;
    assert_eq!(closed["status"], json!("closed"));
    let after_close = stat("2026-10-04T12:00:00Z", "2026-10-04T15:00:00Z").await;
    assert_eq!(data(&after_close)["usage"], json!({ "zonesUsed": 3 }));
    harness.finish().await;
}

#[tokio::test]
async fn stat_reads_only_the_buckets_inside_the_period_and_any_number_of_samples() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": null })).await;
    let account = text(&opened["accountId"]);
    data(&zone_in(&harness, &account, "one.example").await);
    data(&zone_in(&harness, &account, "two.example").await);
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().advance(Duration::hours(1));
    data(&zone_in(&harness, &account, "three.example").await);
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().set(at("2026-10-04T22:00:00Z"));
    data(&zone_in(&harness, &account, "four.example").await);
    data(&zone_in(&harness, &account, "five.example").await);
    run_job(&harness, COLLECT_JOB).await;
    harness.clock().set(at("2026-10-05T00:05:00Z"));
    run_job(&harness, AGGREGATE_JOB).await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT resolution || ' ' || to_char(bucket AT TIME ZONE 'UTC', 'MM-DD HH24:MI') || ' ' || max::text \
                 FROM stats_agg WHERE key = 'dns.zones' AND subject = '{account}' ORDER BY resolution, bucket"
            )
        )
        .await,
        [
            "day 10-04 00:00 5",
            "hour 10-04 12:00 2",
            "hour 10-04 13:00 3",
            "hour 10-04 22:00 5"
        ]
    );

    let stat = |from: &str, to: &str| {
        let params = json!({ "externalId": "svc-1", "from": from, "to": to });
        let harness = &harness;
        async move { data(&call(harness, "stat", params, None).await)["usage"].clone() }
    };
    for (from, to, used) in [
        ("2026-10-04T12:00:00Z", "2026-10-04T15:00:00Z", 3),
        ("2026-10-03T21:00:00Z", "2026-10-04T21:00:00Z", 3),
        ("2026-10-04T12:30:00Z", "2026-10-04T13:30:00Z", 3),
        ("2026-10-04T13:30:00Z", "2026-10-04T22:30:00Z", 5),
        ("2026-10-04T00:00:00Z", "2026-10-05T00:00:00Z", 5),
    ] {
        assert_eq!(
            stat(from, to).await,
            json!({ "zonesUsed": used }),
            "{from} {to}"
        );
    }

    rows(
        &harness,
        &format!(
            "INSERT INTO stats_sample (key, subject, at, value) \
             SELECT 'dns.zones', '{account}', timestamptz '2026-09-20T00:00:00Z' + n * interval '1 minute', \
                    CASE WHEN n = 7000 THEN 42 ELSE 1 END \
             FROM generate_series(0, 10499) AS n"
        ),
    )
    .await;
    assert_eq!(
        stat("2026-09-01T00:00:00Z", "2026-10-01T00:00:00Z").await,
        json!({ "zonesUsed": 42 })
    );
    harness.finish().await;
}

#[tokio::test]
async fn suspend_and_resume_switch_the_zones_of_the_account_out_of_the_name_server_and_back() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&opened["accountId"]);
    let mine = text(&data(&zone_in(&harness, &account, "svc.example").await)["id"]);
    let theirs = text(
        &data(
            &zone_in(
                &harness,
                &harness.fixtures().accounts.a1.to_string(),
                "a1.example",
            )
            .await,
        )["id"],
    );
    state_becomes(&harness, &mine, "active").await;
    state_becomes(&harness, &theirs, "active").await;
    assert!(included(&site, "svc.example"));

    let held = act(&harness, "suspend", "svc-1").await;
    assert_eq!(held["status"], json!("suspended"));
    switched_to(&harness, &account, &["svc.example true"]).await;
    served(&site, "svc.example", false).await;
    assert!(included(&site, "a1.example"));
    assert!(site.zone_file("svc.example").exists());
    let listed = harness
        .call(
            Who::Admin,
            "dns.zone.list",
            json!({ "search": "svc.example" }),
        )
        .await;
    assert_eq!(data(&listed)["data"][0]["suspended"], json!(true));

    let edited = harness
        .call(
            Who::Admin,
            "dns.zone.edit",
            json!({ "id": mine, "refresh": 7200, "retry": 1800, "expire": 1209600, "minimum": 300 }),
        )
        .await;
    data(&edited);
    let late = text(&data(&zone_in(&harness, &account, "late.example").await)["id"]);
    assert_eq!(
        suspended(&harness, &account).await,
        ["late.example true", "svc.example true"]
    );

    let back = act(&harness, "resume", "svc-1").await;
    assert_eq!(back["status"], json!("active"));
    switched_to(
        &harness,
        &account,
        &["late.example false", "svc.example false"],
    )
    .await;
    state_becomes(&harness, &mine, "active").await;
    state_becomes(&harness, &late, "active").await;
    served(&site, "svc.example", true).await;
    served(&site, "late.example", true).await;
    assert!(
        std::fs::read_to_string(site.zone_file("svc.example"))
            .expect("the zone file")
            .contains(" 7200 1800 1209600 300 )")
    );
    assert!(included(&site, "a1.example"));
    harness.finish().await;
}

#[tokio::test]
async fn an_account_resume_keeps_the_zones_of_a_suspended_service_out_of_the_name_server() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&opened["accountId"]);
    let zone = text(&data(&zone_in(&harness, &account, "svc.example").await)["id"]);
    state_becomes(&harness, &zone, "active").await;

    account_call(&harness, "suspend", &account).await;
    switched_to(&harness, &account, &["svc.example true"]).await;
    served(&site, "svc.example", false).await;
    account_call(&harness, "resume", &account).await;
    resumed(&account).await;
    assert_eq!(suspended(&harness, &account).await, ["svc.example false"]);
    served(&site, "svc.example", true).await;

    act(&harness, "suspend", "svc-1").await;
    served(&site, "svc.example", false).await;
    account_call(&harness, "suspend", &account).await;
    account_call(&harness, "resume", &account).await;
    resumed(&account).await;
    assert_eq!(suspended(&harness, &account).await, ["svc.example true"]);
    assert!(!included(&site, "svc.example"));

    account_call(&harness, "suspend", &account).await;
    act(&harness, "resume", "svc-1").await;
    stays(&harness, &account, &["svc.example true"]).await;
    let created = data(&zone_in(&harness, &account, "while.example").await).clone();
    assert_eq!(created["suspended"], json!(true));
    account_call(&harness, "resume", &account).await;
    resumed(&account).await;
    assert_eq!(
        suspended(&harness, &account).await,
        ["svc.example false", "while.example false"]
    );
    served(&site, "svc.example", true).await;
    served(&site, "while.example", true).await;
    harness.finish().await;
}

#[tokio::test]
async fn close_takes_the_zones_out_and_a_new_service_brings_them_back() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&opened["accountId"]);
    let zone = text(&data(&zone_in(&harness, &account, "svc.example").await)["id"]);
    state_becomes(&harness, &zone, "active").await;

    let closed = act(&harness, "close", "svc-1").await;
    assert_eq!(closed["status"], json!("closed"));
    switched_to(&harness, &account, &["svc.example true"]).await;
    served(&site, "svc.example", false).await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT state FROM keel_account WHERE id = '{account}'")
        )
        .await,
        ["suspended"]
    );
    assert_eq!(
        rows(&harness, "SELECT count(*)::text FROM dns_zone").await,
        ["1"]
    );

    let again = open(&harness, "svc-2", "acc-7", json!({ "zoneLimit": 2 })).await;
    assert_eq!(again["accountId"], json!(account));
    assert_eq!(again["state"], json!({ "zonesUsed": 1 }));
    switched_to(&harness, &account, &["svc.example false"]).await;
    state_becomes(&harness, &zone, "active").await;
    served(&site, "svc.example", true).await;
    assert_eq!(limit_of(&harness, &account).await, ["2"]);
    harness.finish().await;
}

#[tokio::test]
async fn a_second_service_of_the_kind_keeps_the_zones_served_while_the_first_is_held() {
    let site = Site::new();
    let harness = boot(&site).await;
    let first = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let second = open(&harness, "svc-2", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&first["accountId"]);
    assert_eq!(second["accountId"], json!(account));
    let zone = text(&data(&zone_in(&harness, &account, "svc.example").await)["id"]);
    state_becomes(&harness, &zone, "active").await;
    served(&site, "svc.example", true).await;

    assert_eq!(
        act(&harness, "suspend", "svc-1").await["status"],
        json!("suspended")
    );
    stays(&harness, &account, &["svc.example false"]).await;
    assert!(included(&site, "svc.example"));

    act(&harness, "suspend", "svc-2").await;
    switched_to(&harness, &account, &["svc.example true"]).await;
    served(&site, "svc.example", false).await;

    act(&harness, "resume", "svc-1").await;
    switched_to(&harness, &account, &["svc.example false"]).await;
    served(&site, "svc.example", true).await;

    assert_eq!(
        act(&harness, "close", "svc-1").await["status"],
        json!("closed")
    );
    switched_to(&harness, &account, &["svc.example true"]).await;
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
    switched_to(&harness, &account, &["svc.example false"]).await;
    served(&site, "svc.example", true).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT s.external_id || ' ' || d.held FROM dns_service d \
             JOIN svc_service s ON s.id = d.service_id ORDER BY s.external_id"
        )
        .await,
        ["svc-1 true", "svc-2 false"]
    );

    let third = open(&harness, "svc-3", "acc-7", json!({ "zoneLimit": 5 })).await;
    assert_eq!(third["accountId"], json!(account));
    act(&harness, "close", "svc-2").await;
    stays(&harness, &account, &["svc.example false"]).await;
    assert!(included(&site, "svc.example"));
    harness.finish().await;
}

async fn terms_of(harness: &Harness, account: &str) -> Vec<String> {
    rows(
        harness,
        &format!(
            "SELECT l.value::text || ' ' || coalesce(a.template, '-') FROM keel_limit l \
             JOIN dns_account a ON a.account_id = l.account_id \
             WHERE l.account_id = '{account}' AND l.key = '{KIND}'"
        ),
    )
    .await
}

async fn change(harness: &Harness, external_id: &str, params: Value) {
    let reply = call(
        harness,
        "set_params",
        json!({ "externalId": external_id, "params": params }),
        None,
    )
    .await;
    data(&reply);
}

#[tokio::test]
async fn closing_the_newer_of_two_services_gives_back_the_limit_and_template_of_the_older() {
    let site = Site::new();
    let harness = boot(&site).await;
    let older = open(
        &harness,
        "svc-1",
        "acc-7",
        json!({ "zoneLimit": 10, "template": WEB }),
    )
    .await;
    let account = text(&older["accountId"]);
    assert_eq!(terms_of(&harness, &account).await, [format!("10 {WEB}")]);
    open(
        &harness,
        "svc-2",
        "acc-7",
        json!({ "zoneLimit": 50, "template": EMPTY }),
    )
    .await;
    assert_eq!(terms_of(&harness, &account).await, [format!("50 {EMPTY}")]);

    change(
        &harness,
        "svc-1",
        json!({ "zoneLimit": 20, "template": WEB }),
    )
    .await;
    assert_eq!(terms_of(&harness, &account).await, [format!("50 {EMPTY}")]);
    assert_eq!(
        rows(
            &harness,
            "SELECT s.external_id || ' ' || d.zone_limit || ' ' || d.template FROM dns_service d \
             JOIN svc_service s ON s.id = d.service_id ORDER BY s.external_id"
        )
        .await,
        [format!("svc-1 20 {WEB}"), format!("svc-2 50 {EMPTY}")]
    );

    act(&harness, "suspend", "svc-2").await;
    assert_eq!(terms_of(&harness, &account).await, [format!("20 {WEB}")]);
    act(&harness, "resume", "svc-2").await;
    assert_eq!(terms_of(&harness, &account).await, [format!("50 {EMPTY}")]);

    assert_eq!(
        act(&harness, "close", "svc-2").await["status"],
        json!("closed")
    );
    assert_eq!(terms_of(&harness, &account).await, [format!("20 {WEB}")]);
    let created = harness
        .call(
            Who::Admin,
            "dns.zone.create",
            json!({ "name": "web.example", "account": account, "ip": "192.0.2.7" }),
        )
        .await;
    assert_eq!(data(&created)["template"], json!(WEB));

    act(&harness, "close", "svc-1").await;
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT state FROM keel_account WHERE id = '{account}'")
        )
        .await,
        ["suspended"]
    );
    assert_eq!(terms_of(&harness, &account).await, [format!("20 {WEB}")]);

    open(&harness, "svc-3", "acc-7", json!({ "zoneLimit": null })).await;
    assert_eq!(terms_of(&harness, &account).await, ["null -"]);
    harness.finish().await;
}

#[tokio::test]
async fn calls_on_two_services_of_one_account_settle_the_account_one_after_the_other() {
    let site = Site::new();
    let harness = boot(&site).await;
    for round in 0..4 {
        let customer = format!("acc-{round}");
        let older = format!("svc-{round}-1");
        let newer = format!("svc-{round}-2");
        let opened = open(
            &harness,
            &older,
            &customer,
            json!({ "zoneLimit": 10, "template": EMPTY }),
        )
        .await;
        open(&harness, &newer, &customer, json!({ "zoneLimit": 50 })).await;
        let account = text(&opened["accountId"]);
        let close_key = format!("close-{newer}");
        let (changed, closed) = tokio::join!(
            call(
                &harness,
                "set_params",
                json!({ "externalId": older, "params": { "zoneLimit": 20, "template": WEB } }),
                None,
            ),
            call(
                &harness,
                "close",
                json!({ "externalId": newer }),
                Some(&close_key)
            ),
        );
        data(&changed);
        data(&closed);
        assert_eq!(
            terms_of(&harness, &account).await,
            [format!("20 {WEB}")],
            "round {round}"
        );

        let newest = format!("svc-{round}-3");
        let open_key = format!("open-{newest}");
        let (opened, changed) = tokio::join!(
            call(
                &harness,
                "open",
                opening(&newest, &customer, json!({ "zoneLimit": 30 })),
                Some(&open_key),
            ),
            call(
                &harness,
                "set_params",
                json!({ "externalId": older, "params": { "zoneLimit": 40, "template": EMPTY } }),
                None,
            ),
        );
        data(&opened);
        data(&changed);
        assert_eq!(
            terms_of(&harness, &account).await,
            ["30 -"],
            "round {round}"
        );
    }
    harness.finish().await;
}

#[tokio::test]
async fn a_zone_created_without_a_template_takes_the_default_template_of_the_service() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(
        &harness,
        "svc-1",
        "acc-7",
        json!({ "zoneLimit": 5, "template": WEB }),
    )
    .await;
    let account = text(&opened["accountId"]);

    let defaulted = data(&zone_in(&harness, &account, "web.example").await).clone();
    assert_eq!(defaulted["template"], json!(WEB));
    let records = |zone: Value| {
        let harness = &harness;
        async move {
            rows(
                harness,
                &format!(
                    "SELECT type || ' ' || name FROM dns_record WHERE zone_id = '{}' ORDER BY type, name",
                    text(&zone)
                ),
            )
            .await
        }
    };
    assert_eq!(
        records(defaulted["id"].clone()).await,
        ["CNAME www", "MX @"]
    );
    let form = harness
        .registry()
        .function("dns.zone.create")
        .expect("dns.zone.create is registered")
        .descriptor()
        .clone()
        .ui
        .expect("the zone form");
    let pages: Vec<(&str, bool)> = form
        .pages
        .iter()
        .map(|page| (page.id.as_str(), page.show_if.is_some()))
        .collect();
    assert_eq!(pages, [("main", false), ("owner", true)]);
    assert!(form.pages[0].fields.iter().any(|field| field == "ip"));
    assert!(form.pages[0].fields.iter().any(|field| field == "ip6"));
    for field in ["ip", "ip6"] {
        assert!(
            form.fields.get(field).is_none_or(|ui| ui.show_if.is_none()),
            "{field} is offered whether or not a template is picked"
        );
    }
    let addressed = harness
        .call(
            Who::Admin,
            "dns.zone.create",
            json!({ "name": "addressed.example", "account": account, "ip": "192.0.2.7", "ip6": "2001:db8::7" }),
        )
        .await;
    assert_eq!(data(&addressed)["template"], json!(WEB));
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT type || ' ' || name || ' ' || value FROM dns_record WHERE zone_id = '{}' ORDER BY type, name",
                text(&data(&addressed)["id"])
            ),
        )
        .await,
        [
            "A @ 192.0.2.7",
            "AAAA @ 2001:db8::7",
            "CNAME www addressed.example",
            "MX @ addressed.example"
        ]
    );
    let chosen = harness
        .call(
            Who::Admin,
            "dns.zone.create",
            json!({ "name": "bare.example", "account": account, "template": EMPTY }),
        )
        .await;
    assert_eq!(data(&chosen)["template"], json!(EMPTY));
    assert!(records(data(&chosen)["id"].clone()).await.is_empty());

    let cleared = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": 5, "template": null } }),
        None,
    )
    .await;
    data(&cleared);
    let plain = data(&zone_in(&harness, &account, "plain.example").await).clone();
    assert_eq!(plain["template"], Value::Null);
    assert!(records(plain["id"].clone()).await.is_empty());

    let a1 = data(
        &zone_in(
            &harness,
            &harness.fixtures().accounts.a1.to_string(),
            "a1.example",
        )
        .await,
    )
    .clone();
    assert_eq!(a1["template"], Value::Null);
    harness.finish().await;
}

#[tokio::test]
async fn a_template_the_account_cannot_see_is_refused_on_its_field() {
    let site = Site::new();
    let harness = boot(&site).await;
    let foreign = harness
        .call(
            Who::A1,
            "dns.template.create",
            json!({ "name": "Own", "records": [] }),
        )
        .await;
    let foreign = text(&data(&foreign)["id"]);

    for template in [foreign.clone(), Id::generate().to_string()] {
        let reply = call(
            &harness,
            "open",
            opening(
                "svc-1",
                "acc-7",
                json!({ "zoneLimit": 5, "template": template }),
            ),
            Some(&format!("open-{template}")),
        )
        .await;
        let error = reply.error().expect("the template is refused");
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.fields[0].path, "params.template");
        assert_eq!(
            error.fields[0].key.as_deref(),
            Some("dns.service.error.template")
        );
        assert_eq!(error.fields[0].params["template"], json!(template));
    }
    assert!(
        rows(&harness, "SELECT id FROM svc_service")
            .await
            .is_empty()
    );
    for table in ["dns_account", "dns_service"] {
        assert!(
            rows(&harness, &format!("SELECT account_id FROM {table}"))
                .await
                .is_empty(),
            "{table}"
        );
    }

    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&opened["accountId"]);
    let refused_change = call(
        &harness,
        "set_params",
        json!({ "externalId": "svc-1", "params": { "zoneLimit": 9, "template": foreign } }),
        None,
    )
    .await;
    assert_eq!(
        refused_change
            .error()
            .expect("the template is refused")
            .fields[0]
            .path,
        "params.template"
    );
    assert_eq!(limit_of(&harness, &account).await, ["5"]);
    harness.finish().await;
}

#[tokio::test]
async fn login_answers_a_one_time_key_of_the_customer_on_the_zone_list() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(&harness, "svc-1", "acc-7", json!({ "zoneLimit": 5 })).await;
    let account = text(&opened["accountId"]);
    let by_hand = harness
        .call(
            Who::Admin,
            "keel.principal.create",
            json!({ "login": "by-hand", "level": "user", "account": account }),
        )
        .await;
    data(&by_hand);

    let nobody = call(&harness, "login", json!({ "externalId": "svc-1" }), None).await;
    refused(&nobody, ErrorCode::Conflict, "svc.error.no_principal");
    assert_eq!(nobody.error().unwrap().params["accountId"], json!(account));

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
    let login = call(&harness, "login", json!({ "externalId": "svc-1" }), None).await;
    let url = text(&data(&login)["url"]);
    assert!(url.starts_with("/login?key=kok_"), "{url}");
    let admin = harness.principal(Who::Admin).id;
    assert_eq!(
        rows(
            &harness,
            "SELECT principal_id || ' ' || issued_by || ' ' || redirect || ' ' || (extract(epoch FROM expires_at - created_at))::int FROM auth_login_key"
        )
        .await,
        [format!("{customer} {admin} /f/dns.zone.list 60")]
    );

    act(&harness, "close", "svc-1").await;
    let closed = call(&harness, "login", json!({ "externalId": "svc-1" }), None).await;
    refused(&closed, ErrorCode::Conflict, "svc.error.closed");
    harness.finish().await;
}

#[tokio::test]
async fn the_catalogue_and_the_handshake_offer_dns_zones_with_its_schemas() {
    let site = Site::new();
    let harness = boot(&site).await;
    let kinds: Vec<&String> = harness
        .registry()
        .contributions()
        .of::<InstanceInfoSource>()
        .flat_map(|source| source.service_kinds())
        .collect();
    assert_eq!(kinds, [KIND]);

    let catalogue = harness.call(Who::Admin, "svc.kinds", json!({})).await;
    let entries = data(&catalogue)["data"].as_array().expect("the entries");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["kind"], json!(KIND));
    assert_eq!(entry["schemaVersion"], json!(1));
    assert_eq!(entry["title"], json!("dns.service.zones.title"));
    assert_eq!(entry["params"]["required"], json!(["zoneLimit"]));
    let limit = &entry["params"]["properties"]["zoneLimit"];
    assert_eq!(limit["type"], json!(["integer", "null"]));
    assert_eq!(limit["minimum"], json!(0));
    assert_eq!(limit["maximum"], json!(100_000));
    assert_eq!(limit["x-ui"], json!({ "control": "slider" }));
    let template = &entry["params"]["properties"]["template"];
    assert_eq!(
        template["x-keel"],
        json!({ "options": "dns.template.options" })
    );
    assert_eq!(template["x-ui"], json!({ "control": "select" }));
    assert_eq!(
        entry["state"]["properties"]["zonesUsed"]["type"],
        json!("integer")
    );
    assert_eq!(entry["state"]["required"], json!(["zonesUsed"]));
    assert_eq!(
        entry["account"]["required"],
        json!(["origin", "externalId"])
    );
    assert_eq!(entry["functions"]["open"], json!("svc.dns.zones.open"));
    assert_eq!(entry["functions"]["login"], json!("svc.dns.zones.login"));
    harness.finish().await;
}

#[tokio::test]
async fn deleting_the_account_of_a_service_forgets_its_dns_settings() {
    let site = Site::new();
    let harness = boot(&site).await;
    let opened = open(
        &harness,
        "svc-1",
        "acc-7",
        json!({ "zoneLimit": 5, "template": WEB }),
    )
    .await;
    let account = text(&opened["accountId"]);
    let zone = text(&data(&zone_in(&harness, &account, "svc.example").await)["id"]);
    state_becomes(&harness, &zone, "active").await;
    act(&harness, "suspend", "svc-1").await;

    let queued = harness
        .call(Who::Admin, "keel.account.delete", json!({ "id": account }))
        .await;
    let task = text(&data(&queued)["task"]["id"]);
    eventually("the account is deleted", async || {
        rows(
            &harness,
            &format!("SELECT state FROM task_job WHERE id = '{task}'"),
        )
        .await
            == ["done"]
    })
    .await;
    for table in ["dns_account", "dns_service"] {
        assert!(
            rows(&harness, &format!("SELECT account_id FROM {table}"))
                .await
                .is_empty(),
            "{table}"
        );
    }
    assert!(rows(&harness, "SELECT id FROM dns_zone").await.is_empty());
    assert!(
        rows(&harness, "SELECT id FROM svc_service")
            .await
            .is_empty()
    );
    harness.finish().await;
}
