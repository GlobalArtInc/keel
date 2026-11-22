use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::types::Via;
use keel_system::cron::api::{CronJob, JobRun, Outcome, Scheduler};
use keel_system::cron::{
    ERROR_NOT_LEADER, ERROR_RUNNING, ERROR_UNKNOWN_JOB, FUNCTIONS, IDEMPOTENCY_JOB,
    IDEMPOTENCY_SCHEDULE, JOB_LIST, JOB_RUN, LOCK_KEY, MENU_PATH, MODULE, OWNER_LOGIN, RUN_LIST,
};
use keel_testkit::{CallOptions, Harness, Reply, Who, tokio};

const START: &str = "2026-09-15T10:00:00Z";
const FAST: &str = "probe.fast";
const SLOW: &str = "probe.slow";
const INVOKER: &str = "probe.invoker";
const BROKEN: &str = "probe.broken";
const PATIENCE: Duration = Duration::from_secs(15);
const QUIET: Duration = Duration::from_millis(2500);

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct Identity {
    login: String,
    level: Level,
    via: Via,
    account: Id,
    request_id: String,
}

static RUNS: Mutex<Vec<(String, Id, DateTime<Utc>)>> = Mutex::new(Vec::new());
static ANSWERS: Mutex<BTreeMap<Id, Value>> = Mutex::new(BTreeMap::new());
static LET_GO: Mutex<Vec<Id>> = Mutex::new(Vec::new());

fn record(run: &JobRun) {
    RUNS.lock().unwrap_or_else(PoisonError::into_inner).push((
        run.job().to_owned(),
        run.id(),
        run.scheduled_at(),
    ));
}

fn recorded(run: Id) -> Option<(String, DateTime<Utc>)> {
    RUNS.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(_, id, _)| *id == run)
        .map(|(job, _, at)| (job.clone(), *at))
}

fn let_go(run: Id) {
    LET_GO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(run);
}

fn let_go_of(run: Id) -> bool {
    LET_GO
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&run)
}

async fn hold(run: Id) {
    while !let_go_of(run) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn answer(run: Id) -> Option<Value> {
    ANSWERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&run)
        .cloned()
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["cron"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(CronJob::new(FAST, "*/5 * * * *", |run| async move {
        record(&run);
        Ok(())
    }))
    .contribute(CronJob::new(SLOW, "0 * * * *", |run| async move {
        record(&run);
        hold(run.id()).await;
        Ok(())
    }))
    .contribute(CronJob::new(INVOKER, "*/10 * * * *", |run| async move {
        record(&run);
        let answered = run.invoke("probe.whoami", json!({})).await?;
        ANSWERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(run.id(), answered);
        Ok(())
    }))
    .contribute(CronJob::new(BROKEN, "0 0 * * *", |run| async move {
        record(&run);
        Err(KeelError::conflict("probe.error.broken").param("job", run.job()))
    }))
    .function(
        function::<Nothing, Identity>("probe.whoami")
            .kind(Kind::Get)
            .level(Level::Admin)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let principal = call.principal();
                Ok(Identity {
                    login: principal.login.clone(),
                    level: principal.level,
                    via: principal.via.clone(),
                    account: principal.account_id,
                    request_id: call.request_id().to_owned(),
                })
            }),
    );
    Ok(())
}

static UNPARSABLE: ModuleDef = ModuleDef {
    id: "broken",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["cron"],
    migrations: None,
    init: unparsable_init,
};

fn unparsable_init(r: &mut Registrar) -> Result<()> {
    r.contribute(CronJob::new("broken.daily", "every day", |_run| async {
        Ok(())
    }));
    Ok(())
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn scheduler(harness: &Harness) -> Arc<Scheduler> {
    harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler")
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused(reply: &Reply, status: u16, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), status, "{error:?}");
    assert_eq!(error.key, key, "{error:?}");
}

async fn boot_at(start: &str) -> Harness {
    Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .clock(at(start))
        .boot()
        .await
}

async fn boot() -> Harness {
    boot_at(START).await
}

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_leader(harness: &Harness) {
    let scheduler = scheduler(harness);
    wait_until("the leader takes the lock", || {
        let scheduler = scheduler.clone();
        async move { scheduler.is_leader() }
    })
    .await;
}

fn runs_sql(job: &str) -> String {
    format!(
        "SELECT id || ' ' || outcome || ' ' || to_char(scheduled_at AT TIME ZONE 'UTC', 'HH24:MI:SS') || ' ' || to_char(started_at AT TIME ZONE 'UTC', 'HH24:MI:SS') || ' ' || coalesce(to_char(finished_at AT TIME ZONE 'UTC', 'HH24:MI:SS'), '-') || ' ' || coalesce(error, '-') FROM cron_run WHERE job = '{job}' ORDER BY started_at, id"
    )
}

async fn runs(harness: &Harness, job: &str) -> Vec<String> {
    harness.database().rows(&runs_sql(job)).await
}

async fn wait_runs(harness: &Harness, job: &str, outcome: Outcome, count: usize) -> Vec<String> {
    let needle = format!(" {outcome} ");
    wait_until(&format!("{count} {outcome} runs of {job}"), || async {
        runs(harness, job)
            .await
            .iter()
            .filter(|row| row.contains(&needle))
            .count()
            >= count
    })
    .await;
    runs(harness, job).await
}

fn without_id(row: &str) -> &str {
    row.split_once(' ').map(|(_, rest)| rest).unwrap_or(row)
}

fn id_of(row: &str) -> Id {
    row.split_once(' ')
        .map(|(id, _)| id)
        .unwrap_or(row)
        .parse()
        .expect("a run id")
}

fn the_running(rows: &[String]) -> Id {
    let running: Vec<&String> = rows
        .iter()
        .filter(|row| row.contains(" running "))
        .collect();
    assert_eq!(running.len(), 1, "{rows:?}");
    id_of(running[0])
}

async fn job_row(harness: &Harness, job: &str) -> Value {
    let listed = harness.call(Who::Admin, JOB_LIST, json!({})).await;
    data(&listed)["data"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == json!(job))
        .cloned()
        .unwrap_or_else(|| panic!("no row for {job} in {:?}", listed.data()))
}

#[tokio::test]
async fn a_job_runs_when_the_clock_passes_its_next_occurrence_and_records_an_ok_run() {
    let harness = boot().await;
    wait_leader(&harness).await;
    let fresh = job_row(&harness, FAST).await;
    assert_eq!(fresh["module"], json!("probe"));
    assert_eq!(fresh["schedule"], json!("*/5 * * * *"));
    assert_eq!(fresh["effective"], json!("*/5 * * * *"));
    assert_eq!(fresh["lastRunAt"], Value::Null);
    assert_eq!(fresh["lastOutcome"], Value::Null);
    assert_eq!(fresh["nextRunAt"], json!("2026-09-15T10:05:00Z"));
    tokio::time::sleep(QUIET).await;
    assert!(runs(&harness, FAST).await.is_empty());

    harness.clock().set(at("2026-09-15T10:05:00Z"));
    let rows = wait_runs(&harness, FAST, Outcome::Ok, 1).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(without_id(&rows[0]), "ok 10:05:00 10:05:00 10:05:00 -");
    let run = id_of(&rows[0]);
    assert_eq!(
        recorded(run),
        Some((FAST.to_owned(), at("2026-09-15T10:05:00Z")))
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        runs(&harness, FAST).await.len(),
        1,
        "the job ran more than once"
    );
    assert!(runs(&harness, SLOW).await.is_empty());

    let ran = job_row(&harness, FAST).await;
    assert_eq!(ran["lastRunAt"], json!("2026-09-15T10:05:00Z"));
    assert_eq!(ran["lastOutcome"], json!("ok"));
    assert_eq!(ran["nextRunAt"], json!("2026-09-15T10:10:00Z"));
    let listed = harness
        .call(
            Who::Admin,
            JOB_LIST,
            json!({ "filters": [{ "key": "lastOutcome", "op": "eq", "value": "ok" }], "sort": { "key": "id", "dir": "desc" } }),
        )
        .await;
    let page = data(&listed);
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["id"], json!(FAST));
    let searched = harness
        .call(Who::Admin, JOB_LIST, json!({ "search": "slo" }))
        .await;
    assert_eq!(data(&searched)["total"], json!(1));
    assert_eq!(data(&searched)["data"][0]["id"], json!(SLOW));
    let all = harness
        .call(
            Who::Admin,
            JOB_LIST,
            json!({ "sort": { "key": "id", "dir": "asc" } }),
        )
        .await;
    let ids: Vec<&str> = data(&all)["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(ids, [IDEMPOTENCY_JOB, BROKEN, FAST, INVOKER, SLOW]);
    harness.finish().await;
}

#[tokio::test]
async fn a_job_still_running_is_not_started_again_and_the_occurrence_is_recorded_skipped() {
    let harness = boot().await;
    wait_leader(&harness).await;
    harness.clock().set(at("2026-09-15T11:00:00Z"));
    let first = the_running(&wait_runs(&harness, SLOW, Outcome::Running, 1).await);
    let running = scheduler(&harness);
    wait_until("the quick jobs of the minute finish", || {
        let running = running.clone();
        async move { running.running() == [SLOW] }
    })
    .await;
    assert_eq!(
        job_row(&harness, SLOW).await["lastOutcome"],
        json!("running")
    );

    harness.clock().set(at("2026-09-15T12:00:00Z"));
    wait_runs(&harness, SLOW, Outcome::Skipped, 1).await;
    let_go(first);
    let rows = wait_runs(&harness, SLOW, Outcome::Ok, 1).await;
    let outcomes: Vec<&str> = rows.iter().map(|row| without_id(row)).collect();
    assert_eq!(
        outcomes,
        [
            "ok 11:00:00 11:00:00 12:00:00 -",
            "skipped 12:00:00 12:00:00 12:00:00 -"
        ]
    );
    wait_until("every job of the hour finishes", || {
        let running = running.clone();
        async move { running.running().is_empty() }
    })
    .await;
    assert_eq!(
        recorded(id_of(&rows[0])).map(|(job, _)| job),
        Some(SLOW.to_owned())
    );
    assert_eq!(recorded(id_of(&rows[1])), None);

    harness.clock().set(at("2026-09-15T13:00:00Z"));
    let_go(the_running(
        &wait_runs(&harness, SLOW, Outcome::Running, 1).await,
    ));
    wait_runs(&harness, SLOW, Outcome::Ok, 2).await;
    assert_eq!(runs(&harness, SLOW).await.len(), 3);
    harness.finish().await;
}

#[tokio::test]
async fn a_run_left_unfinished_at_start_is_marked_interrupted() {
    let first = boot().await;
    wait_leader(&first).await;
    first.clock().set(at("2026-09-15T11:00:00Z"));
    wait_runs(&first, SLOW, Outcome::Running, 1).await;
    let database = first.stop().await;
    let rows = database.rows(&runs_sql(SLOW)).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        without_id(&rows[0]),
        "interrupted 11:00:00 11:00:00 11:00:00 -"
    );

    let crashed = Id::generate();
    let inserted = database
        .rows(&format!(
            "INSERT INTO cron_run (id, job, scheduled_at, started_at, finished_at, outcome, error) VALUES ('{crashed}', '{FAST}', '2026-09-15T11:10:00Z', '2026-09-15T11:10:00Z', NULL, 'running', NULL) RETURNING id"
        ))
        .await;
    assert_eq!(inserted, [crashed.to_string()]);

    let second = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .clock(at("2026-09-15T11:30:00Z"))
        .database(database)
        .boot()
        .await;
    wait_leader(&second).await;
    let rows = wait_runs(&second, FAST, Outcome::Interrupted, 1).await;
    let outcomes: Vec<&str> = rows.iter().map(|row| without_id(row)).collect();
    assert_eq!(
        outcomes,
        [
            "ok 10:05:00 11:00:00 11:00:00 -",
            "interrupted 11:10:00 11:10:00 11:30:00 -"
        ]
    );
    assert_eq!(id_of(&rows[1]), crashed);
    assert_eq!(
        job_row(&second, FAST).await["lastOutcome"],
        json!("interrupted")
    );
    second.finish().await;
}

#[tokio::test]
async fn two_kernels_on_one_database_elect_one_leader_and_the_other_takes_over_after_a_stop() {
    let first = boot().await;
    let second = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .clock(at(START))
        .database(first.database_handle())
        .boot()
        .await;
    wait_leader(&first).await;
    tokio::time::sleep(QUIET).await;
    assert!(!scheduler(&second).is_leader());
    let held = first
        .database()
        .rows(&format!(
            "SELECT count(*)::text FROM pg_locks WHERE locktype = 'advisory' AND granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) AND ((classid::bigint << 32) | objid::bigint) = {LOCK_KEY}"
        ))
        .await;
    assert_eq!(held, ["1"]);

    let follower = second
        .call(Who::Admin, JOB_RUN, json!({ "id": FAST }))
        .await;
    refused(&follower, 503, ERROR_NOT_LEADER);
    second.clock().set(at("2026-09-15T10:05:00Z"));
    tokio::time::sleep(QUIET).await;
    assert!(
        runs(&second, FAST).await.is_empty(),
        "the follower ran a job"
    );

    first.clock().set(at("2026-09-15T10:05:00Z"));
    let rows = wait_runs(&first, FAST, Outcome::Ok, 1).await;
    assert_eq!(without_id(&rows[0]), "ok 10:05:00 10:05:00 10:05:00 -");

    let database = first.stop().await;
    wait_leader(&second).await;
    let led = second
        .call(Who::Admin, JOB_RUN, json!({ "id": FAST }))
        .await;
    let started = data(&led);
    assert_eq!(started["id"], json!(FAST));
    assert_eq!(started["scheduledAt"], json!("2026-09-15T10:05:00Z"));
    wait_runs(&second, FAST, Outcome::Ok, 2).await;
    second.clock().set(at("2026-09-15T10:10:00Z"));
    let rows = wait_runs(&second, FAST, Outcome::Ok, 3).await;
    let outcomes: Vec<&str> = rows.iter().map(|row| without_id(row)).collect();
    assert_eq!(
        outcomes,
        [
            "ok 10:05:00 10:05:00 10:05:00 -",
            "ok 10:05:00 10:05:00 10:05:00 -",
            "ok 10:10:00 10:10:00 10:10:00 -",
        ]
    );
    second.release().await;
    database.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn a_cron_overrides_entry_replaces_the_contributed_schedule_and_an_unparsable_one_is_ignored()
{
    use keel_system::cron::OVERRIDES_KEY;

    let harness = Harness::new()
        .module(&MODULE)
        .module(&keel_system::settings::MODULE)
        .module(&PROBE)
        .clock(at(START))
        .boot()
        .await;
    wait_leader(&harness).await;
    let set = |value: Value| {
        harness.call(
            Who::Admin,
            "settings.set",
            json!({ "key": OVERRIDES_KEY, "value": value }),
        )
    };
    let listed = harness
        .call(Who::Admin, "settings.get", json!({ "key": OVERRIDES_KEY }))
        .await;
    assert_eq!(data(&listed)["default"], json!({}));
    assert_eq!(data(&listed)["level"], json!("admin"));
    let mistyped = set(json!({ FAST: 5 })).await;
    assert_eq!(mistyped.status(), 422, "{:?}", mistyped.error());

    data(&set(json!({ FAST: "*/2 * * * *" })).await);
    harness.clock().set(at("2026-09-15T10:02:00Z"));
    let rows = wait_runs(&harness, FAST, Outcome::Ok, 1).await;
    assert_eq!(without_id(&rows[0]), "ok 10:02:00 10:02:00 10:02:00 -");
    let overridden = job_row(&harness, FAST).await;
    assert_eq!(overridden["schedule"], json!("*/5 * * * *"));
    assert_eq!(overridden["effective"], json!("*/2 * * * *"));
    assert_eq!(overridden["nextRunAt"], json!("2026-09-15T10:04:00Z"));

    data(&set(json!({ FAST: "every day" })).await);
    let ignored = job_row(&harness, FAST).await;
    assert_eq!(ignored["effective"], json!("*/5 * * * *"));
    assert_eq!(ignored["nextRunAt"], json!("2026-09-15T10:05:00Z"));
    harness.clock().set(at("2026-09-15T10:04:00Z"));
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        runs(&harness, FAST).await.len(),
        1,
        "the unparsable override ran the job"
    );
    harness.clock().set(at("2026-09-15T10:05:00Z"));
    let rows = wait_runs(&harness, FAST, Outcome::Ok, 2).await;
    assert_eq!(without_id(&rows[1]), "ok 10:05:00 10:05:00 10:05:00 -");
    harness.finish().await;
}

#[tokio::test]
async fn cron_job_run_starts_a_job_now_and_answers_conflict_while_it_runs() {
    let harness = boot().await;
    wait_leader(&harness).await;
    let started = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": SLOW }))
        .await;
    let started = data(&started);
    assert_eq!(started["id"], json!(SLOW));
    assert_eq!(started["scheduledAt"], json!(START));
    let run: Id = started["run"]
        .as_str()
        .expect("a run id")
        .parse()
        .expect("a ulid");
    let again = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": SLOW }))
        .await;
    refused(&again, 409, ERROR_RUNNING);
    assert_eq!(again.error().expect("an error").params["id"], json!(SLOW));
    assert_eq!(scheduler(&harness).running(), [SLOW]);
    let_go(run);
    assert_eq!(
        scheduler(&harness).wait(run).await.expect("the run ends"),
        Outcome::Ok
    );
    let rows = runs(&harness, SLOW).await;
    assert_eq!(rows, [format!("{run} ok 10:00:00 10:00:00 10:00:00 -")]);
    assert_eq!(recorded(run), Some((SLOW.to_owned(), at(START))));

    let failing = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": BROKEN }))
        .await;
    let failing: Id = data(&failing)["run"]
        .as_str()
        .expect("a run id")
        .parse()
        .expect("a ulid");
    assert_eq!(
        scheduler(&harness)
            .wait(failing)
            .await
            .expect("the run ends"),
        Outcome::Failed
    );
    let rows = runs(&harness, BROKEN).await;
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].contains("failed 10:00:00 10:00:00 10:00:00 conflict: probe.error.broken"),
        "{}",
        rows[0]
    );
    assert_eq!(
        job_row(&harness, BROKEN).await["lastOutcome"],
        json!("failed")
    );

    let unknown = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": "probe.nothing" }))
        .await;
    refused(&unknown, 404, ERROR_UNKNOWN_JOB);
    let reseller = harness
        .call(Who::Reseller, JOB_RUN, json!({ "id": SLOW }))
        .await;
    assert_eq!(reseller.status(), 403);
    assert!(runs(&harness, SLOW).await.len() == 1);

    let listed = harness.call(Who::Admin, RUN_LIST, json!({})).await;
    let page = data(&listed);
    assert_eq!(page["total"], json!(2));
    assert_eq!(page["data"][0]["job"], json!(BROKEN));
    assert_eq!(page["data"][0]["outcome"], json!("failed"));
    assert_eq!(page["data"][0]["scheduledAt"], json!(START));
    assert_eq!(page["data"][1]["job"], json!(SLOW));
    assert_eq!(page["data"][1]["outcome"], json!("ok"));
    assert_eq!(page["data"][1]["error"], Value::Null);
    let failed = harness
        .call(
            Who::Admin,
            RUN_LIST,
            json!({ "filters": [{ "key": "outcome", "op": "eq", "value": "failed" }] }),
        )
        .await;
    assert_eq!(data(&failed)["total"], json!(1));
    assert_eq!(data(&failed)["data"][0]["id"], json!(failing.to_string()));
    let of_slow = harness
        .call(
            Who::Admin,
            RUN_LIST,
            json!({ "filters": [{ "key": "job", "op": "eq", "value": SLOW }], "sort": { "key": "startedAt", "dir": "asc" } }),
        )
        .await;
    assert_eq!(data(&of_slow)["total"], json!(1));
    assert_eq!(data(&of_slow)["data"][0]["id"], json!(run.to_string()));
    let searched = harness
        .call(Who::Admin, RUN_LIST, json!({ "search": "brok" }))
        .await;
    assert_eq!(data(&searched)["total"], json!(1));
    harness.finish().await;
}

#[tokio::test]
async fn a_products_job_invokes_a_function_as_the_owner_through_the_run() {
    let harness = boot().await;
    wait_leader(&harness).await;
    let started = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": INVOKER }))
        .await;
    let run: Id = data(&started)["run"]
        .as_str()
        .expect("a run id")
        .parse()
        .expect("a ulid");
    assert_eq!(
        scheduler(&harness).wait(run).await.expect("the run ends"),
        Outcome::Ok
    );
    let answered = answer(run).expect("the job stored the answer");
    assert_eq!(answered["login"], json!(OWNER_LOGIN));
    assert_eq!(answered["level"], json!("owner"));
    assert_eq!(answered["via"], json!("internal"));
    assert_eq!(answered["account"], json!(Id::ROOT_ACCOUNT.to_string()));
    assert_eq!(
        answered["requestId"],
        json!(format!("cron-{}", run.to_string().to_lowercase()))
    );
    let rows = runs(&harness, INVOKER).await;
    assert_eq!(rows, [format!("{run} ok 10:00:00 10:00:00 10:00:00 -")]);
    harness.finish().await;
}

#[tokio::test]
async fn the_hourly_job_of_cron_prunes_the_expired_idempotency_records() {
    let harness = boot().await;
    wait_leader(&harness).await;
    let job = job_row(&harness, IDEMPOTENCY_JOB).await;
    assert_eq!(job["module"], json!("cron"));
    assert_eq!(job["schedule"], json!(IDEMPOTENCY_SCHEDULE));
    assert_eq!(job["nextRunAt"], json!("2026-09-15T10:25:00Z"));
    let reseller = harness.principal(Who::Reseller).clone();
    for (index, key) in ["first", "second"].into_iter().enumerate() {
        if index > 0 {
            harness
                .clock()
                .advance(keel_sdk::chrono::Duration::minutes(20));
        }
        let created = harness
            .call_with(
                &reseller,
                "keel.account.create",
                json!({ "kind": "user" }),
                CallOptions {
                    idempotency_key: Some(key.to_owned()),
                    ..CallOptions::default()
                },
            )
            .await;
        data(&created);
    }
    let keys = || {
        harness
            .database()
            .rows("SELECT key FROM keel_idempotency ORDER BY key")
    };
    assert_eq!(keys().await, ["first", "second"]);
    assert!(runs(&harness, IDEMPOTENCY_JOB).await.is_empty());

    harness.clock().set(at("2026-09-16T10:10:00Z"));
    let rows = wait_runs(&harness, IDEMPOTENCY_JOB, Outcome::Ok, 1).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        keys().await,
        ["second"],
        "the record that expired at 10:00 is gone and the one living until 10:20 stays"
    );

    harness.clock().set(at("2026-09-16T10:20:00Z"));
    let started = harness
        .call(Who::Admin, JOB_RUN, json!({ "id": IDEMPOTENCY_JOB }))
        .await;
    let run: Id = data(&started)["run"]
        .as_str()
        .expect("a run id")
        .parse()
        .expect("a ulid");
    assert_eq!(
        scheduler(&harness).wait(run).await.expect("the run ends"),
        Outcome::Ok
    );
    assert!(keys().await.is_empty());
    assert_eq!(runs(&harness, IDEMPOTENCY_JOB).await.len(), 2);
    harness.finish().await;
}

#[tokio::test]
async fn a_schedule_that_does_not_parse_fails_the_start_naming_the_job() {
    let Err(error) = Harness::new()
        .module(&MODULE)
        .module(&UNPARSABLE)
        .try_boot()
        .await
    else {
        panic!("the composition booted with an unparsable schedule");
    };
    let text = error.to_string();
    assert!(text.contains("module cron: start failed"), "{text}");
    assert!(text.contains("cron.error.schedule"), "{text}");
    assert!(text.contains("broken.daily"), "{text}");
    assert!(text.contains("every day"), "{text}");
    assert!(text.contains("expected five fields, found 2"), "{text}");
}

#[tokio::test]
async fn the_cron_descriptors_match_their_snapshots_and_the_reseller_is_answered_403() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "cron", "testkit"]);
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let cron: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("cron."))
            .collect();
        if level < Level::Admin {
            assert!(cron.is_empty(), "{level}: {cron:?}");
        } else {
            let mut expected = FUNCTIONS.to_vec();
            expected.sort_unstable();
            assert_eq!(cron, expected, "{level}");
        }
    }
    for func in FUNCTIONS {
        let reseller = harness
            .call(Who::Reseller, func, json!({ "id": "probe.fast" }))
            .await;
        assert_eq!(reseller.status(), 403, "{func}: {:?}", reseller.error());
        let user = harness
            .call(Who::A1, func, json!({ "id": "probe.fast" }))
            .await;
        assert_eq!(user.status(), 403, "{func}: {:?}", user.error());
    }
    let alone = harness.call(Who::Admin, JOB_LIST, json!({})).await;
    assert_eq!(data(&alone)["total"], json!(1));
    assert_eq!(data(&alone)["data"][0]["id"], json!(IDEMPOTENCY_JOB));
    let owner = harness.call(Who::Owner, RUN_LIST, json!({})).await;
    assert_eq!(data(&owner)["total"], json!(0));

    let http = harness.http().await;
    let booted = http.get(Who::Admin, "/api/v1/boot").await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    let boot = booted.json();
    let node = crate::menu::node(&boot, MENU_PATH)
        .unwrap_or_else(|| panic!("no cron node in {}", boot["menu"]));
    assert_eq!(node["func"], json!(JOB_LIST));
    assert_eq!(node["label"], json!("cron.menu.schedule"));
    assert_eq!(node["kind"], json!("list"));
    let reseller = http.get(Who::Reseller, "/api/v1/boot").await;
    assert!(
        !reseller.body.contains(JOB_LIST),
        "a reseller sees the cron node: {}",
        reseller.body
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
