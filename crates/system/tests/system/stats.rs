use std::time::Duration;

use keel_sdk::StatCollector;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::limit::Unit;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use keel_sdk::types::ErrorCode;
use keel_system::cron::ERROR_RUNNING;
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::stats::{
    AGGREGATE_JOB, COLLECT_JOB, ERROR_KEY, ERROR_SUBJECT, KEYS, MODULE, PRUNE_JOB, SERIES,
};
use keel_testkit::{Harness, Reply, Who, tokio};

const PATIENCE: Duration = Duration::from_secs(10);
const LOAD: &str = "probe.load";
const ITEMS: &str = "probe.items";
const KV_LOAD: &str = "probe_load";
const KV_ITEMS: &str = "probe_items";
const UNITS: &str = "units.memory";

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(StatCollector::new(
        LOAD,
        Unit::Count,
        Duration::from_secs(300),
        |db| async move {
            let row = db
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT (value #>> '{}')::DOUBLE PRECISION AS load FROM keel_kv WHERE key = $1",
                    [DbValue::from(KV_LOAD)],
                ))
                .await?;
            Ok(match row {
                Some(row) => row.try_get::<f64>("", "load")?,
                None => 0.0,
            })
        },
    ))
    .contribute(StatCollector::per_account(
        ITEMS,
        Unit::Count,
        Duration::from_secs(60),
        |db| async move {
            let rows = db
                .query_all(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT a.id, COALESCE((SELECT (v.value ->> a.id)::DOUBLE PRECISION \
                     FROM keel_kv v WHERE v.key = $1), 0) AS items FROM keel_account a",
                    [DbValue::from(KV_ITEMS)],
                ))
                .await?;
            let mut items = Vec::new();
            for row in rows {
                let account: String = row.try_get("", "id")?;
                let count: f64 = row.try_get("", "items")?;
                items.push((account.parse().expect("an account id"), count));
            }
            Ok(items)
        },
    ));
    Ok(())
}

static UNITS_PROBE: ModuleDef = ModuleDef {
    id: "units",
    version: "0.1.0",
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init: init_units,
};

fn init_units(r: &mut Registrar) -> Result<()> {
    r.contribute(StatCollector::per_subject(
        UNITS,
        Unit::Bytes,
        Duration::from_secs(300),
        |_| async {
            Ok(vec![
                ("nginx".to_owned(), 7_340_032.0),
                ("postfix".to_owned(), 1_048_576.0),
            ])
        },
    ));
    Ok(())
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

async fn composed(now: &str) -> Harness {
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .clock(at(now))
        .boot()
        .await
}

async fn put(harness: &Harness, key: &str, value: Value) {
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
            [DbValue::from(key), DbValue::Json(Some(Box::new(value)))],
        ))
        .await
        .expect("the probe value is stored");
}

async fn sample(harness: &Harness, key: &str, subject: &str, at: &str, value: f64) {
    harness
        .db()
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO stats_sample (key, subject, at, value) VALUES ($1, $2, $3, $4)",
            [
                DbValue::from(key),
                DbValue::from(subject),
                DbValue::from(self::at(at)),
                DbValue::from(value),
            ],
        ))
        .await
        .expect("the sample is stored");
}

async fn run(harness: &Harness, job: &str) {
    let scheduler = harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler");
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let started = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{job} did not start in time"
        );
        if !scheduler.is_leader() {
            tokio::time::sleep(Duration::from_millis(20)).await;
            continue;
        }
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

async fn collect_at(harness: &Harness, now: &str) {
    harness.clock().set(at(now));
    run(harness, COLLECT_JOB).await;
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

async fn series(harness: &Harness, who: Who, params: Value) -> Reply {
    harness.call(who, SERIES, params).await
}

fn points(reply: &Reply) -> Vec<(String, f64, f64, f64, u64)> {
    data(reply)["points"]
        .as_array()
        .expect("the points")
        .iter()
        .map(|point| {
            (
                point["at"].as_str().expect("an instant").to_owned(),
                point["value"].as_f64().expect("a value"),
                point["min"].as_f64().expect("a minimum"),
                point["max"].as_f64().expect("a maximum"),
                point["count"].as_u64().expect("a count"),
            )
        })
        .collect()
}

fn values(reply: &Reply) -> Vec<f64> {
    points(reply).into_iter().map(|point| point.1).collect()
}

fn refused(reply: &Reply, code: ErrorCode, key: &str) {
    let error = reply.error().expect("the call is refused");
    assert_eq!(error.code, code, "{error:?}");
    assert_eq!(error.key, key);
}

async fn keys(harness: &Harness, who: Who) -> Vec<String> {
    let reply = harness.call(who, KEYS, json!({})).await;
    data(&reply)["keys"]
        .as_array()
        .expect("the keys")
        .iter()
        .map(|key| {
            format!(
                "{} {} {} {}",
                key["key"].as_str().expect("a key"),
                key["unit"].as_str().expect("a unit"),
                key["intervalSeconds"].as_u64().expect("an interval"),
                key["scope"].as_str().expect("a scope"),
            )
        })
        .collect()
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

#[tokio::test]
async fn a_collector_is_sampled_on_its_interval_and_aggregated_into_hours_and_days() {
    let harness = composed("2026-09-24T00:00:00Z").await;
    put(&harness, KV_LOAD, json!(10)).await;
    run(&harness, COLLECT_JOB).await;
    put(&harness, KV_LOAD, json!(15)).await;
    collect_at(&harness, "2026-09-24T00:01:00Z").await;
    put(&harness, KV_LOAD, json!(20)).await;
    collect_at(&harness, "2026-09-24T00:05:00Z").await;
    put(&harness, KV_LOAD, json!(30)).await;
    collect_at(&harness, "2026-09-24T00:10:00Z").await;
    put(&harness, KV_LOAD, json!(50)).await;
    collect_at(&harness, "2026-09-24T01:00:00Z").await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT to_char(at AT TIME ZONE 'UTC', 'HH24:MI') || ' ' || value::text \
                 FROM stats_sample WHERE key = '{LOAD}' ORDER BY at"
            ),
        )
        .await,
        ["00:00 10", "00:05 20", "00:10 30", "01:00 50"]
    );
    let accounts = rows(
        &harness,
        "SELECT count(*)::text FROM keel_account WHERE kind <> 'system'",
    )
    .await;
    assert_eq!(
        rows(
            &harness,
            "SELECT to_char(at AT TIME ZONE 'UTC', 'HH24:MI') || ' ' || value::text \
             FROM stats_sample WHERE key = 'keel.accounts' AND subject = '' ORDER BY at",
        )
        .await,
        [
            format!("00:00 {}", accounts[0]),
            format!("01:00 {}", accounts[0])
        ]
    );
    let principals = rows(&harness, "SELECT count(*)::text FROM keel_principal").await;
    assert_eq!(
        rows(
            &harness,
            "SELECT value::text FROM stats_sample WHERE key = 'keel.principals' ORDER BY at",
        )
        .await,
        [principals[0].clone(), principals[0].clone()]
    );

    harness.clock().set(at("2026-09-25T00:05:00Z"));
    run(&harness, AGGREGATE_JOB).await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT resolution || ' ' || to_char(bucket AT TIME ZONE 'UTC', 'MM-DD HH24:MI') || ' ' \
                 || min::text || ' ' || max::text || ' ' || sum::text || ' ' || count::text \
                 FROM stats_agg WHERE key = '{LOAD}' ORDER BY resolution, bucket"
            ),
        )
        .await,
        [
            "day 09-24 00:00 10 50 110 4",
            "hour 09-24 00:00 10 30 60 3",
            "hour 09-24 01:00 50 50 50 1",
        ]
    );
    let range = |resolution: &str| {
        json!({
            "key": LOAD,
            "from": "2026-09-24T00:00:00Z",
            "to": "2026-09-25T00:00:00Z",
            "resolution": resolution,
        })
    };
    let raw = series(&harness, Who::Admin, range("raw")).await;
    assert_eq!(values(&raw), [10.0, 20.0, 30.0, 50.0]);
    assert_eq!(data(&raw)["unit"], json!("count"));
    assert!(data(&raw).get("subject").is_none());
    let hours = series(&harness, Who::Admin, range("hour")).await;
    assert_eq!(
        points(&hours),
        [
            ("2026-09-24T00:00:00Z".to_owned(), 20.0, 10.0, 30.0, 3),
            ("2026-09-24T01:00:00Z".to_owned(), 50.0, 50.0, 50.0, 1),
        ]
    );
    let days = series(&harness, Who::Admin, range("day")).await;
    assert_eq!(
        points(&days),
        [("2026-09-24T00:00:00Z".to_owned(), 27.5, 10.0, 50.0, 4)]
    );
    assert_eq!(
        keys(&harness, Who::Admin).await,
        [
            "keel.accounts count 3600 installation",
            "keel.principals count 3600 installation",
            "probe.items count 60 account",
            "probe.load count 300 installation",
        ]
    );
    harness.finish().await;
}

#[tokio::test]
async fn old_samples_roll_over_into_buckets_and_are_pruned() {
    let harness = composed("2026-09-25T00:00:00Z").await;
    for (at, value) in [
        ("2026-06-01T10:00:00Z", 1.0),
        ("2026-06-01T10:30:00Z", 3.0),
        ("2026-09-10T10:00:00Z", 5.0),
        ("2026-09-10T10:30:00Z", 7.0),
        ("2026-09-24T10:00:00Z", 9.0),
    ] {
        sample(&harness, LOAD, "", at, value).await;
    }
    harness.clock().set(at("2026-09-25T00:20:00Z"));
    run(&harness, PRUNE_JOB).await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT to_char(at AT TIME ZONE 'UTC', 'MM-DD HH24:MI') FROM stats_sample \
                 WHERE key = '{LOAD}' ORDER BY at"
            ),
        )
        .await,
        ["09-24 10:00"]
    );
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT resolution || ' ' || to_char(bucket AT TIME ZONE 'UTC', 'MM-DD HH24:MI') \
                 || ' ' || sum::text || ' ' || count::text \
                 FROM stats_agg WHERE key = '{LOAD}' ORDER BY resolution, bucket"
            ),
        )
        .await,
        [
            "day 06-01 00:00 4 2",
            "day 09-10 00:00 12 2",
            "day 09-24 00:00 9 1",
            "hour 09-10 10:00 12 2",
            "hour 09-24 10:00 9 1",
        ]
    );
    let range = |resolution: &str| {
        json!({
            "key": LOAD,
            "from": "2026-05-01T00:00:00Z",
            "to": "2026-09-25T00:00:00Z",
            "resolution": resolution,
        })
    };
    assert_eq!(
        values(&series(&harness, Who::Admin, range("raw")).await),
        [9.0]
    );
    assert_eq!(
        values(&series(&harness, Who::Admin, range("hour")).await),
        [6.0, 9.0]
    );
    assert_eq!(
        values(&series(&harness, Who::Admin, range("day")).await),
        [2.0, 6.0, 9.0]
    );

    harness.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn the_retention_of_samples_and_hours_follows_the_settings() {
    use keel_system::stats::{HOUR_DAYS_KEY, RAW_DAYS_KEY};

    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::settings::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .clock(at("2026-09-24T00:00:00Z"))
        .boot()
        .await;
    for at in ["2026-09-22T10:00:00Z", "2026-09-24T10:00:00Z"] {
        sample(&harness, LOAD, "", at, 2.0).await;
    }
    let set = |key: &'static str, value: Value| {
        harness.call(
            Who::Admin,
            "settings.set",
            json!({ "key": key, "value": value }),
        )
    };
    let refused = set(RAW_DAYS_KEY, json!(0)).await;
    assert_eq!(refused.status(), 422, "{:?}", refused.error());
    data(&set(RAW_DAYS_KEY, json!(1)).await);
    data(&set(HOUR_DAYS_KEY, json!(2)).await);
    harness.clock().set(at("2026-09-25T00:20:00Z"));
    run(&harness, PRUNE_JOB).await;
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT to_char(at AT TIME ZONE 'UTC', 'MM-DD HH24:MI') FROM stats_sample \
                 WHERE key = '{LOAD}' ORDER BY at"
            ),
        )
        .await,
        ["09-24 10:00"]
    );
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT resolution || ' ' || to_char(bucket AT TIME ZONE 'UTC', 'MM-DD HH24:MI') \
                 FROM stats_agg WHERE key = '{LOAD}' ORDER BY resolution, bucket"
            ),
        )
        .await,
        ["day 09-22 00:00", "day 09-24 00:00", "hour 09-24 10:00",]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_per_account_series_is_scoped_to_the_callers_account() {
    let harness = composed("2026-09-24T00:00:00Z").await;
    let accounts = harness.fixtures().accounts.clone();
    put(
        &harness,
        KV_ITEMS,
        json!({
            accounts.a1.to_string(): 3,
            accounts.a2.to_string(): 5,
            accounts.unrelated.to_string(): 7,
        }),
    )
    .await;
    run(&harness, COLLECT_JOB).await;
    let window = |subject: Option<Id>| {
        let mut params = json!({
            "key": ITEMS,
            "from": "2026-09-24T00:00:00Z",
            "to": "2026-09-24T01:00:00Z",
            "resolution": "raw",
        });
        if let Some(subject) = subject {
            params["subject"] = json!(subject.to_string());
        }
        params
    };

    let own = series(&harness, Who::A1, window(None)).await;
    assert_eq!(values(&own), [3.0]);
    assert_eq!(data(&own)["subject"], json!(accounts.a1.to_string()));
    assert_eq!(
        values(&series(&harness, Who::A1, window(Some(accounts.a1))).await),
        [3.0]
    );
    assert_eq!(
        values(&series(&harness, Who::A2, window(None)).await),
        [5.0]
    );
    for subject in [accounts.a2, accounts.unrelated, accounts.reseller] {
        let reply = series(&harness, Who::A1, window(Some(subject))).await;
        refused(&reply, ErrorCode::NotFound, ERROR_SUBJECT);
        assert_eq!(
            reply.error().expect("refused").params["subject"],
            json!(subject.to_string())
        );
    }
    let mut backwards = window(None);
    backwards["to"] = json!("2026-09-23T00:00:00Z");
    let reply = series(&harness, Who::A1, backwards).await;
    assert_eq!(reply.status(), 422, "{:?}", reply.error());
    let mut foreign = window(None);
    foreign["subject"] = json!("not-an-account");
    refused(
        &series(&harness, Who::A1, foreign).await,
        ErrorCode::NotFound,
        ERROR_SUBJECT,
    );

    assert_eq!(
        values(&series(&harness, Who::Reseller, window(Some(accounts.a2))).await),
        [5.0]
    );
    refused(
        &series(&harness, Who::Reseller, window(Some(accounts.unrelated))).await,
        ErrorCode::NotFound,
        ERROR_SUBJECT,
    );
    assert_eq!(
        values(&series(&harness, Who::Admin, window(Some(accounts.unrelated))).await),
        [7.0]
    );

    let installation = json!({
        "key": LOAD,
        "from": "2026-09-24T00:00:00Z",
        "to": "2026-09-24T01:00:00Z",
        "resolution": "raw",
    });
    refused(
        &series(&harness, Who::A1, installation.clone()).await,
        ErrorCode::NotFound,
        ERROR_KEY,
    );
    refused(
        &series(&harness, Who::Reseller, installation.clone()).await,
        ErrorCode::NotFound,
        ERROR_KEY,
    );
    let mut aimed = installation.clone();
    aimed["subject"] = json!(accounts.a1.to_string());
    refused(
        &series(&harness, Who::Admin, aimed).await,
        ErrorCode::NotFound,
        ERROR_SUBJECT,
    );
    assert_eq!(
        keys(&harness, Who::A1).await,
        ["probe.items count 60 account"]
    );
    assert_eq!(
        keys(&harness, Who::Reseller).await,
        ["probe.items count 60 account"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_per_subject_series_answers_each_named_subject_to_an_admin_alone() {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&UNITS_PROBE)
        .clock(at("2026-09-24T00:00:00Z"))
        .boot()
        .await;
    collect_at(&harness, "2026-09-24T00:05:00Z").await;
    let window = |subject: Option<&str>| {
        let mut params = json!({
            "key": UNITS,
            "from": "2026-09-24T00:00:00Z",
            "to": "2026-09-24T01:00:00Z",
            "resolution": "raw",
        });
        if let Some(subject) = subject {
            params["subject"] = json!(subject);
        }
        params
    };
    let nginx = series(&harness, Who::Admin, window(Some("nginx"))).await;
    assert_eq!(values(&nginx), [7_340_032.0]);
    assert_eq!(data(&nginx)["subject"], json!("nginx"));
    assert_eq!(
        values(&series(&harness, Who::Admin, window(Some("postfix"))).await),
        [1_048_576.0]
    );
    assert!(
        values(&series(&harness, Who::Admin, window(None)).await).is_empty(),
        "no sample is kept without a subject"
    );
    refused(
        &series(&harness, Who::A1, window(Some("nginx"))).await,
        ErrorCode::NotFound,
        ERROR_KEY,
    );
    assert!(
        keys(&harness, Who::Admin)
            .await
            .contains(&"units.memory bytes 300 installation".to_owned())
    );
    assert!(keys(&harness, Who::A1).await.is_empty());
    harness.finish().await;
}
