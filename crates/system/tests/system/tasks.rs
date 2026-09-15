use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use keel_sdk::prelude::*;
use keel_sdk::system::{Frame, TaskRunner, TaskSpec};
use keel_sdk::types::{TaskProgress, TaskState, Via};
use keel_sdk::{Registrar, TaskHandle};
use keel_system::tasks::api::Drain;
use keel_system::tasks::{ERROR_FINISHED, ERROR_INTERRUPTED, FRAME_KIND, MODULE, log_path};
use keel_testkit::{Harness, Who, tokio};
use tokio::sync::broadcast::Receiver;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Counted {
    counted: u64,
    login: String,
    via: Via,
    task: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Paced {
    steps: u64,
    pace: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Stepped {
    steps: u64,
    from: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Cursor {
    next: u64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Sealed {
    label: String,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    password: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Unsealed {
    label: String,
    length: usize,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Guarded {
    label: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Passed {
    label: String,
    task: Option<Id>,
}

static TIMELINE: Mutex<Vec<(String, &'static str, Instant)>> = Mutex::new(Vec::new());

fn mark(task: &TaskHandle, what: &'static str) {
    TIMELINE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((task.id().to_string(), what, Instant::now()));
}

fn moment(id: Id, what: &str) -> Instant {
    TIMELINE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(task, mark, _)| *task == id.to_string() && *mark == what)
        .map(|(_, _, at)| *at)
        .unwrap_or_else(|| panic!("no {what} mark for {id}"))
}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["task"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Counted>("probe.count")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let principal = call.principal();
                let Some(task) = call.task() else {
                    return Ok(Counted {
                        counted: 0,
                        login: principal.login.clone(),
                        via: principal.via.clone(),
                        task: None,
                    });
                };
                for step in 1..=3 {
                    task.progress(step, Some(3), Some(&format!("step {step}")))
                        .await?;
                    if step < 3 {
                        task.log(&format!("counted {step}")).await?;
                    }
                }
                Ok(Counted {
                    counted: 3,
                    login: principal.login.clone(),
                    via: principal.via.clone(),
                    task: Some(task.id()),
                })
            }),
    )
    .function(paced("probe.paced", "slow", true))
    .function(paced("probe.stubborn", "stubborn", false))
    .function(paced("probe.quick", "quick", true))
    .function(
        function::<Sealed, Unsealed>("probe.sealed")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Sealed>| async move {
                Ok(Unsealed {
                    label: call.params().label.clone(),
                    length: call.params().password.expose().len(),
                })
            }),
    )
    .function(
        function::<Guarded, Passed>("probe.guarded")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Guarded>| async move {
                Ok(Passed {
                    label: call.params().label.clone(),
                    task: call.task().map(TaskHandle::id),
                })
            }),
    )
    .within("probe.guarded", |call: Call<Value>| async move {
        call.tx()?;
        let label = &call.params()["label"];
        if *label == json!("veto") {
            return Err(KeelError::conflict("probe.error.veto")
                .param("label", label.clone())
                .field("label"));
        }
        Ok(())
    });
    Ok(())
}

fn paced(name: &str, class: &str, resumable: bool) -> FunctionDef {
    let builder = function::<Paced, Stepped>(name)
        .kind(Kind::Action)
        .level(Level::User)
        .unowned()
        .class(class);
    let builder = if resumable {
        builder.resumable()
    } else {
        builder
    };
    builder.handler(|call: Call<Paced>| async move {
        let task = call
            .task()
            .ok_or_else(|| KeelError::internal("probe.error.inline"))?;
        let steps = call.params().steps;
        let pace = Duration::from_millis(call.params().pace);
        let from = task
            .resume_from::<Cursor>()?
            .map(|cursor| cursor.next)
            .unwrap_or_default();
        mark(task, "start");
        for step in from..steps {
            tokio::time::sleep(pace).await;
            if task.is_cancelled() {
                task.checkpoint(&Cursor { next: step }).await?;
                mark(task, "stopped");
                return Err(task.cancelled());
            }
            task.progress(step + 1, Some(steps), Some(&format!("step {step}")))
                .await?;
            task.log(&format!("step {step}")).await?;
            task.checkpoint(&Cursor { next: step + 1 }).await?;
        }
        mark(task, "end");
        Ok(Stepped { steps, from })
    })
}

fn runner(harness: &Harness) -> Arc<dyn TaskRunner> {
    harness
        .registry()
        .services()
        .get::<dyn TaskRunner>()
        .expect("the tasks module provides the runner")
}

fn drain(harness: &Harness) -> Arc<Drain> {
    harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain")
}

fn listen(harness: &Harness) -> Receiver<Frame> {
    harness.live_events()
}

fn spec(harness: &Harness, who: Who, function: &str, params: Value, class: &str) -> TaskSpec {
    TaskSpec {
        function: function.to_owned(),
        params,
        principal: harness.principal(who).clone(),
        class: class.to_owned(),
        resumable: !matches!(
            function,
            "probe.stubborn" | "probe.count" | "probe.sealed" | "probe.guarded"
        ),
        request_id: Some(format!("req-{function}")),
    }
}

async fn wait_final(harness: &Harness, id: Id) -> TaskState {
    tokio::time::timeout(Duration::from_secs(30), drain(harness).wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

async fn wait_progress(harness: &Harness, id: Id, at_least: u64) {
    let sql = format!("SELECT progress_done::text FROM task_job WHERE id = '{id}'");
    for _ in 0..300 {
        let rows = harness.database().rows(&sql).await;
        if rows
            .first()
            .and_then(|done| done.parse::<u64>().ok())
            .is_some_and(|done| done >= at_least)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job {id} did not reach {at_least} steps");
}

async fn frames_until(
    listener: &mut Receiver<Frame>,
    id: Id,
    state: TaskState,
) -> Vec<TaskProgress> {
    let mut seen = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), listener.recv())
            .await
            .expect("a frame arrives in time")
            .expect("the bus is open");
        assert_eq!(frame.kind, FRAME_KIND);
        let progress: TaskProgress =
            keel_sdk::serde_json::from_value(frame.data).expect("a progress frame");
        if progress.id != id {
            continue;
        }
        let done = progress.state == state;
        seen.push(progress);
        if done {
            return seen;
        }
    }
}

async fn column(harness: &Harness, id: Id, column: &str) -> String {
    harness
        .database()
        .rows(&format!(
            "SELECT coalesce({column}::text, 'null') FROM task_job WHERE id = '{id}'"
        ))
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no row for {id}"))
}

fn log_lines(data_dir: &Path, id: Id) -> Vec<String> {
    std::fs::read_to_string(log_path(data_dir, id))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            line.split_once(' ')
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or_default()
        })
        .collect()
}

async fn boot() -> Harness {
    Harness::new().module(&MODULE).module(&PROBE).boot().await
}

#[tokio::test]
async fn a_spawned_job_runs_under_the_stored_principal_and_records_progress_steps_and_log_lines() {
    let harness = boot().await;
    let mut listener = listen(&harness);
    let a1 = harness.principal(Who::A1).clone();
    let id = runner(&harness)
        .spawn(spec(&harness, Who::A1, "probe.count", json!({}), "default"))
        .await
        .expect("the job is spawned");
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);
    assert_eq!(
        runner(&harness).state(id).await.expect("the state"),
        TaskState::Done
    );

    let result: Value = keel_sdk::serde_json::from_str(&column(&harness, id, "result_json").await)
        .expect("the result is stored");
    assert_eq!(result["counted"], json!(3));
    assert_eq!(result["login"], json!(a1.login));
    assert_eq!(result["via"], json!("internal"));
    assert_eq!(result["task"], json!(id.to_string()));
    assert_eq!(
        column(&harness, id, "principal_id").await,
        a1.id.to_string()
    );
    assert_eq!(
        column(&harness, id, "account_id").await,
        a1.account_id.to_string()
    );
    assert_eq!(column(&harness, id, "class").await, "default");
    assert_eq!(column(&harness, id, "request_id").await, "req-probe.count");
    assert_eq!(column(&harness, id, "progress_done").await, "3");
    assert_eq!(column(&harness, id, "progress_total").await, "3");
    assert_eq!(column(&harness, id, "message").await, "step 3");
    assert_ne!(column(&harness, id, "started_at").await, "null");
    assert_ne!(column(&harness, id, "finished_at").await, "null");

    let steps = harness
        .database()
        .rows(&format!(
            "SELECT seq || ' ' || done || '/' || coalesce(total::text, '-') || ' ' || coalesce(message, '') FROM task_step WHERE job_id = '{id}' ORDER BY seq"
        ))
        .await;
    assert_eq!(steps, ["1 1/3 step 1", "2 2/3 step 2", "3 3/3 step 3"]);
    assert_eq!(
        log_lines(harness.data_dir(), id),
        ["counted 1", "counted 2"]
    );
    let stamped = std::fs::read_to_string(log_path(harness.data_dir(), id)).expect("the log");
    let stamp = harness
        .clock()
        .now()
        .to_rfc3339_opts(keel_sdk::chrono::SecondsFormat::Millis, true);
    assert!(stamped.starts_with(&stamp), "{stamped}");

    let frames = frames_until(&mut listener, id, TaskState::Done).await;
    let states: Vec<(TaskState, u64)> = frames
        .iter()
        .map(|frame| (frame.state, frame.done))
        .collect();
    assert_eq!(
        states,
        [
            (TaskState::Queued, 0),
            (TaskState::Running, 0),
            (TaskState::Running, 1),
            (TaskState::Running, 2),
            (TaskState::Running, 3),
            (TaskState::Done, 3),
        ]
    );
    assert!(frames.iter().all(|frame| frame.function == "probe.count"));
    let inline = harness.call(Who::A1, "probe.count", json!({})).await;
    assert_eq!(inline.data().expect("inline")["task"], json!(null));
    harness.finish().await;
}

#[tokio::test]
async fn a_cancelled_job_stops_at_its_next_checkpoint_and_ends_cancelled() {
    let harness = boot().await;
    let runner = runner(&harness);
    let running = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.paced",
            json!({ "steps": 40, "pace": 50 }),
            "slow",
        ))
        .await
        .expect("spawned");
    let queued = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.paced",
            json!({ "steps": 2, "pace": 10 }),
            "slow",
        ))
        .await
        .expect("spawned");
    wait_progress(&harness, running, 2).await;
    assert_eq!(
        runner.state(queued).await.expect("state"),
        TaskState::Queued
    );
    runner
        .cancel(queued)
        .await
        .expect("a queued job is cancelled at once");
    assert_eq!(
        runner.state(queued).await.expect("state"),
        TaskState::Cancelled
    );
    let again = runner.cancel(queued).await.expect_err("it has ended");
    assert_eq!(again.key(), ERROR_FINISHED);
    assert_eq!(again.params()["state"], json!("cancelled"));

    runner.cancel(running).await.expect("the flag is set");
    assert_eq!(wait_final(&harness, running).await, TaskState::Cancelled);
    let done: u64 = column(&harness, running, "progress_done")
        .await
        .parse()
        .expect("a number");
    assert!((2..40).contains(&done), "{done}");
    let checkpoint: Value =
        keel_sdk::serde_json::from_str(&column(&harness, running, "checkpoint_json").await)
            .expect("the checkpoint");
    assert_eq!(checkpoint["next"], json!(done));
    assert_eq!(column(&harness, running, "error_json").await, "null");
    assert_eq!(column(&harness, running, "result_json").await, "null");
    assert_eq!(
        log_lines(harness.data_dir(), running).len(),
        usize::try_from(done).expect("fits")
    );
    let stopped = moment(running, "stopped");
    let started = moment(running, "start");
    assert!(stopped > started);
    let unknown = runner
        .cancel(Id::generate())
        .await
        .expect_err("no such job");
    assert_eq!(unknown.key(), "task.error.not_found");
    harness.finish().await;
}

#[tokio::test]
async fn a_job_running_at_stop_is_interrupted_and_a_resumable_one_restarts_from_its_checkpoint() {
    let dir = tempfile::tempdir().expect("a data directory");
    let first = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .boot()
        .await;
    let runner = runner(&first);
    let resumable = runner
        .spawn(spec(
            &first,
            Who::A1,
            "probe.paced",
            json!({ "steps": 6, "pace": 100 }),
            "slow",
        ))
        .await
        .expect("spawned");
    let stubborn = runner
        .spawn(spec(
            &first,
            Who::A2,
            "probe.stubborn",
            json!({ "steps": 6, "pace": 100 }),
            "stubborn",
        ))
        .await
        .expect("spawned");
    wait_progress(&first, resumable, 2).await;
    wait_progress(&first, stubborn, 2).await;
    let database = first.stop().await;

    let states = database
        .rows(&format!(
            "SELECT id || ' ' || state || ' ' || progress_done || ' ' || coalesce(error_json->>'key', '-') FROM task_job WHERE id IN ('{resumable}', '{stubborn}') ORDER BY id"
        ))
        .await;
    for line in &states {
        assert!(line.contains(" interrupted "), "{line}");
        assert!(line.ends_with(ERROR_INTERRUPTED), "{line}");
    }
    let resumed_from: u64 = database
        .rows(&format!(
            "SELECT progress_done::text FROM task_job WHERE id = '{resumable}'"
        ))
        .await[0]
        .parse()
        .expect("a number");
    assert!((2..6).contains(&resumed_from), "{resumed_from}");
    assert_eq!(
        database
            .rows(&format!(
                "SELECT checkpoint_json->>'next' FROM task_job WHERE id = '{resumable}'"
            ))
            .await,
        [resumed_from.to_string()]
    );

    let crashed = database
        .rows(&format!(
            "UPDATE task_job SET state = 'running', error_json = NULL, finished_at = NULL WHERE id IN ('{resumable}', '{stubborn}') RETURNING id"
        ))
        .await;
    assert_eq!(crashed.len(), 2);

    let second = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .database(database)
        .boot()
        .await;
    assert_eq!(wait_final(&second, resumable).await, TaskState::Done);
    assert_eq!(wait_final(&second, stubborn).await, TaskState::Interrupted);
    let result: Value =
        keel_sdk::serde_json::from_str(&column(&second, resumable, "result_json").await)
            .expect("the result");
    assert_eq!(result["from"], json!(resumed_from));
    assert_eq!(result["steps"], json!(6));
    assert_eq!(column(&second, resumable, "progress_done").await, "6");
    assert_eq!(column(&second, resumable, "error_json").await, "null");
    let lines = log_lines(dir.path(), resumable);
    let expected: Vec<String> = (0..6).map(|step| format!("step {step}")).collect();
    assert_eq!(lines, expected);

    let stubborn_error: Value =
        keel_sdk::serde_json::from_str(&column(&second, stubborn, "error_json").await)
            .expect("the error");
    assert_eq!(stubborn_error["key"], json!(ERROR_INTERRUPTED));
    assert_eq!(column(&second, stubborn, "result_json").await, "null");
    let stubborn_done: u64 = column(&second, stubborn, "progress_done")
        .await
        .parse()
        .expect("a number");
    assert!((2..6).contains(&stubborn_done), "{stubborn_done}");
    assert_eq!(
        log_lines(dir.path(), stubborn).len(),
        usize::try_from(stubborn_done).expect("fits")
    );
    second.finish().await;
}

#[tokio::test]
async fn stop_lets_a_running_job_reach_its_checkpoint_and_leaves_queued_jobs_for_the_next_start() {
    let dir = tempfile::tempdir().expect("a data directory");
    let first = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .boot()
        .await;
    let runner = runner(&first);
    let running = runner
        .spawn(spec(
            &first,
            Who::A1,
            "probe.paced",
            json!({ "steps": 8, "pace": 100 }),
            "slow",
        ))
        .await
        .expect("spawned");
    let waiting = runner
        .spawn(spec(
            &first,
            Who::A1,
            "probe.paced",
            json!({ "steps": 3, "pace": 10 }),
            "slow",
        ))
        .await
        .expect("spawned");
    wait_progress(&first, running, 1).await;
    let database = first.stop().await;

    let stopped = moment(running, "stopped");
    let states = database
        .rows(&format!(
            "SELECT id || ' ' || state || ' ' || progress_done || ' ' || coalesce(checkpoint_json->>'next', '-') FROM task_job WHERE id IN ('{running}', '{waiting}') ORDER BY created_at, id"
        ))
        .await;
    let interrupted = states
        .iter()
        .find(|line| line.starts_with(&running.to_string()))
        .expect("the running row");
    let parts: Vec<&str> = interrupted.split(' ').collect();
    assert_eq!(parts[1], "interrupted");
    assert_eq!(
        parts[2], parts[3],
        "the checkpoint matches the progress: {interrupted}"
    );
    let done: u64 = parts[2].parse().expect("a number");
    assert!((1..8).contains(&done), "{interrupted}");
    assert_eq!(
        states
            .iter()
            .find(|line| line.starts_with(&waiting.to_string()))
            .map(|line| line.split(' ').nth(1).unwrap_or_default()),
        Some("queued")
    );
    assert!(stopped.elapsed() < Duration::from_secs(10));

    let second = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .database(database)
        .boot()
        .await;
    assert_eq!(wait_final(&second, waiting).await, TaskState::Done);
    assert_eq!(wait_final(&second, running).await, TaskState::Done);
    tokio::time::timeout(Duration::from_secs(30), drain(&second).drain())
        .await
        .expect("the queue drains")
        .expect("the drain is read");
    let expected: Vec<String> = (0..8).map(|step| format!("step {step}")).collect();
    assert_eq!(log_lines(dir.path(), running), expected);
    let result: Value =
        keel_sdk::serde_json::from_str(&column(&second, running, "result_json").await)
            .expect("the result");
    assert_eq!(result["from"], json!(done));
    second.finish().await;
}

#[tokio::test]
async fn jobs_of_two_classes_run_concurrently_while_jobs_of_one_class_run_in_order() {
    let harness = boot().await;
    let runner = runner(&harness);
    let slow_one = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.paced",
            json!({ "steps": 4, "pace": 100 }),
            "slow",
        ))
        .await
        .expect("spawned");
    wait_progress(&harness, slow_one, 1).await;
    let slow_two = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.paced",
            json!({ "steps": 1, "pace": 10 }),
            "slow",
        ))
        .await
        .expect("spawned");
    let quick = runner
        .spawn(spec(
            &harness,
            Who::A2,
            "probe.quick",
            json!({ "steps": 1, "pace": 10 }),
            "quick",
        ))
        .await
        .expect("spawned");
    assert_eq!(wait_final(&harness, quick).await, TaskState::Done);
    assert_eq!(
        runner.state(slow_one).await.expect("state"),
        TaskState::Running
    );
    assert_eq!(
        runner.state(slow_two).await.expect("state"),
        TaskState::Queued
    );
    assert_eq!(wait_final(&harness, slow_one).await, TaskState::Done);
    assert_eq!(wait_final(&harness, slow_two).await, TaskState::Done);
    assert!(moment(quick, "end") < moment(slow_one, "end"));
    assert!(moment(slow_two, "start") >= moment(slow_one, "end"));
    tokio::time::timeout(Duration::from_secs(30), drain(&harness).drain())
        .await
        .expect("the queue drains")
        .expect("the drain is read");
    harness.finish().await;
}

#[tokio::test]
async fn jobs_of_one_class_spawned_within_one_clock_tick_run_in_spawn_order() {
    let harness = boot().await;
    let runner = runner(&harness);
    let mut ids = Vec::new();
    for _ in 0..6 {
        let id = runner
            .spawn(spec(
                &harness,
                Who::A1,
                "probe.paced",
                json!({ "steps": 1, "pace": 10 }),
                "slow",
            ))
            .await
            .expect("spawned");
        ids.push(id);
    }
    for id in &ids {
        assert_eq!(wait_final(&harness, *id).await, TaskState::Done);
    }
    let created: Vec<String> = harness
        .database()
        .rows("SELECT DISTINCT created_at::text FROM task_job")
        .await;
    assert_eq!(created.len(), 1, "{created:?}");
    for pair in ids.windows(2) {
        assert!(
            moment(pair[0], "end") <= moment(pair[1], "start"),
            "{} ran before {}",
            pair[1],
            pair[0]
        );
    }
    tokio::time::timeout(Duration::from_secs(30), drain(&harness).drain())
        .await
        .expect("the queue drains")
        .expect("the drain is read");
    harness.finish().await;
}

#[tokio::test]
async fn two_kernels_on_one_database_run_one_worker_and_the_second_takes_over_when_the_first_stops()
{
    let dir = tempfile::tempdir().expect("a data directory");
    let first = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .boot()
        .await;
    let second = Harness::new()
        .module(&MODULE)
        .module(&PROBE)
        .data_dir(dir.path())
        .database(first.database_handle())
        .boot()
        .await;
    let mut on_first = listen(&first);
    let mut on_second = listen(&second);

    let id = runner(&second)
        .spawn(spec(&second, Who::A1, "probe.count", json!({}), "default"))
        .await
        .expect("spawned through the waiting kernel");
    assert_eq!(wait_final(&second, id).await, TaskState::Done);
    let frames = frames_until(&mut on_first, id, TaskState::Done).await;
    assert_eq!(frames[0].state, TaskState::Running);
    let queued = tokio::time::timeout(Duration::from_secs(5), on_second.recv())
        .await
        .expect("the spawn frame")
        .expect("the bus is open");
    assert_eq!(queued.kind, FRAME_KIND);
    let queued: TaskProgress =
        keel_sdk::serde_json::from_value(queued.data).expect("a progress frame");
    assert_eq!((queued.id, queued.state), (id, TaskState::Queued));
    assert!(
        on_second.try_recv().is_err(),
        "the waiting kernel published nothing beyond the spawn"
    );

    let database = first.stop().await;
    let taken_over = runner(&second)
        .spawn(spec(&second, Who::A2, "probe.count", json!({}), "default"))
        .await
        .expect("spawned after the takeover");
    assert_eq!(wait_final(&second, taken_over).await, TaskState::Done);
    let frames = frames_until(&mut on_second, taken_over, TaskState::Done).await;
    let states: Vec<TaskState> = frames.iter().map(|frame| frame.state).collect();
    assert_eq!(
        states,
        [
            TaskState::Queued,
            TaskState::Running,
            TaskState::Running,
            TaskState::Running,
            TaskState::Running,
            TaskState::Done,
        ]
    );
    second.release().await;
    database.finish().await;
}

fn snapshots_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

struct JobFixture;

#[async_trait]
impl keel_testkit::Fixture for JobFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let (_, principal) = harness
            .fixtures()
            .all()
            .find(|(who, principal)| {
                principal.account_id == account && *who != Who::Guest && *who != Who::Owner
            })
            .unwrap_or_else(|| panic!("no fixture principal lives in account {account}"));
        let id = Id::generate();
        let principal_json = keel_sdk::serde_json::to_string(principal).expect("a principal");
        let inserted = harness
            .database()
            .rows(&format!(
                "INSERT INTO task_job (id, class, function, params_json, params_masked_json, principal_id, principal_json, account_id, state, resumable, progress_done, progress_total, created_at, started_at, finished_at) VALUES ('{id}', 'default', 'probe.count', '{{}}', '{{}}', '{}', '{}', '{account}', 'done', false, 3, 3, now(), now(), now()) RETURNING id",
                principal.id,
                principal_json.replace('\'', "''")
            ))
            .await;
        inserted.into_iter().next().expect("the fixture row")
    }
}

#[tokio::test]
async fn task_get_shows_the_last_step_the_log_tail_and_task_cancel_ends_a_paced_job_cancelled() {
    let harness = boot().await;
    let runner = runner(&harness);
    let counted = runner
        .spawn(spec(&harness, Who::A1, "probe.count", json!({}), "default"))
        .await
        .expect("spawned");
    assert_eq!(wait_final(&harness, counted).await, TaskState::Done);

    let got = harness
        .call(Who::A1, "task.get", json!({ "id": counted }))
        .await;
    let detail = got.data().unwrap_or_else(|| panic!("{:?}", got.error()));
    assert_eq!(detail["id"], json!(counted.to_string()));
    assert_eq!(detail["function"], json!("probe.count"));
    assert_eq!(detail["state"], json!("done"));
    assert_eq!(detail["progress"], json!("3/3"));
    assert_eq!(detail["done"], json!(3));
    assert_eq!(detail["total"], json!(3));
    assert_eq!(detail["message"], json!("step 3"));
    assert_eq!(detail["result"]["counted"], json!(3));
    assert_eq!(detail["params"], json!({}));
    assert_eq!(detail["error"], Value::Null);
    assert!(detail.get("accountId").is_none(), "{detail}");
    let tail = detail["logTail"].as_array().expect("the log tail");
    assert_eq!(tail.len(), 2);
    assert!(
        tail[0]
            .as_str()
            .is_some_and(|line| line.ends_with(" counted 1"))
    );
    assert!(
        tail[1]
            .as_str()
            .is_some_and(|line| line.ends_with(" counted 2"))
    );
    let as_admin = harness
        .call(Who::Admin, "task.get", json!({ "id": counted }))
        .await;
    assert_eq!(
        as_admin.data().expect("the admin reads it")["accountId"],
        json!(harness.principal(Who::A1).account_id.to_string())
    );
    let foreign = harness
        .call(Who::A2, "task.get", json!({ "id": counted }))
        .await;
    assert_eq!(foreign.status(), 404);

    let listed = harness.call(Who::A1, "task.list", json!({})).await;
    let page = listed.data().expect("the list answers");
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["progress"], json!("3/3"));
    assert_eq!(page["data"][0]["state"], json!("done"));
    assert!(page["data"][0].get("accountId").is_none());

    let paced = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.paced",
            json!({ "steps": 40, "pace": 50 }),
            "slow",
        ))
        .await
        .expect("spawned");
    wait_progress(&harness, paced, 1).await;
    let cancelled = harness
        .call(Who::A1, "task.cancel", json!({ "id": paced }))
        .await;
    let answer = cancelled
        .data()
        .unwrap_or_else(|| panic!("{:?}", cancelled.error()));
    assert_eq!(answer["id"], json!(paced.to_string()));
    assert!(
        matches!(answer["state"].as_str(), Some("running" | "cancelled")),
        "{answer}"
    );
    assert_eq!(wait_final(&harness, paced).await, TaskState::Cancelled);
    let again = harness
        .call(Who::A1, "task.cancel", json!({ "id": paced }))
        .await;
    assert_eq!(again.status(), 409);
    assert_eq!(again.error().expect("a conflict").key, ERROR_FINISHED);
    let filtered = harness
        .call(
            Who::A1,
            "task.list",
            json!({ "filters": [{ "key": "state", "op": "eq", "value": "cancelled" }] }),
        )
        .await;
    let page = filtered.data().expect("the filter applies");
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["id"], json!(paced.to_string()));
    harness.finish().await;
}

#[tokio::test]
async fn task_get_masks_the_secret_fields_of_the_parameters_while_the_job_ran_them_raw() {
    let harness = boot().await;
    let runner = runner(&harness);
    let id = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.sealed",
            json!({ "label": "plain", "password": "hunter2" }),
            "default",
        ))
        .await
        .expect("spawned");
    assert_eq!(wait_final(&harness, id).await, TaskState::Done);
    let result: Value = keel_sdk::serde_json::from_str(&column(&harness, id, "result_json").await)
        .expect("the result");
    assert_eq!(result, json!({ "label": "plain", "length": 7 }));
    assert_eq!(
        column(&harness, id, "params_json->>'password'").await,
        "hunter2"
    );
    assert_eq!(
        column(&harness, id, "params_masked_json->>'password'").await,
        "***"
    );
    assert_eq!(
        column(&harness, id, "params_masked_json->>'label'").await,
        "plain"
    );
    for who in [Who::A1, Who::Reseller, Who::Admin] {
        let got = harness.call(who, "task.get", json!({ "id": id })).await;
        let detail = got
            .data()
            .unwrap_or_else(|| panic!("{who}: {:?}", got.error()));
        assert_eq!(
            detail["params"],
            json!({ "label": "plain", "password": "***" }),
            "{who}"
        );
        assert!(!detail.to_string().contains("hunter2"), "{who}: {detail}");
    }
    let unknown = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.nothing",
            json!({ "password": "hunter2" }),
            "default",
        ))
        .await
        .expect_err("an unknown function cannot be spawned");
    assert_eq!(unknown.key(), "keel.error.function_not_found");
    assert_eq!(unknown.params()["function"], json!("probe.nothing"));
    assert_eq!(
        harness
            .database()
            .rows("SELECT count(*)::text FROM task_job WHERE function = 'probe.nothing'")
            .await,
        ["0"]
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_job_vetoed_by_a_within_hook_ends_failed_with_the_field_error() {
    let harness = boot().await;
    let mut listener = listen(&harness);
    let runner = runner(&harness);
    let vetoed = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.guarded",
            json!({ "label": "veto" }),
            "default",
        ))
        .await
        .expect("spawned");
    assert_eq!(wait_final(&harness, vetoed).await, TaskState::Failed);
    let error: Value =
        keel_sdk::serde_json::from_str(&column(&harness, vetoed, "error_json").await)
            .expect("the error is stored");
    assert_eq!(error["code"], json!("validation"));
    assert_eq!(error["fields"][0]["path"], json!("label"));
    assert_eq!(error["fields"][0]["key"], json!("probe.error.veto"));
    assert_eq!(error["fields"][0]["params"]["label"], json!("veto"));
    assert_eq!(column(&harness, vetoed, "result_json").await, "null");
    assert_ne!(column(&harness, vetoed, "finished_at").await, "null");
    let states: Vec<TaskState> = frames_until(&mut listener, vetoed, TaskState::Failed)
        .await
        .iter()
        .map(|frame| frame.state)
        .collect();
    assert_eq!(
        states,
        [TaskState::Queued, TaskState::Running, TaskState::Failed]
    );
    let got = harness
        .call(Who::A1, "task.get", json!({ "id": vetoed }))
        .await;
    let detail = got.data().unwrap_or_else(|| panic!("{:?}", got.error()));
    assert_eq!(detail["state"], json!("failed"));
    assert_eq!(
        detail["error"]["fields"][0]["key"],
        json!("probe.error.veto")
    );
    assert_eq!(detail["result"], json!(null));

    let passed = runner
        .spawn(spec(
            &harness,
            Who::A1,
            "probe.guarded",
            json!({ "label": "fine" }),
            "default",
        ))
        .await
        .expect("spawned");
    assert_eq!(wait_final(&harness, passed).await, TaskState::Done);
    let result: Value =
        keel_sdk::serde_json::from_str(&column(&harness, passed, "result_json").await)
            .expect("the result is stored");
    assert_eq!(
        result,
        json!({ "label": "fine", "task": passed.to_string() })
    );
    assert_eq!(column(&harness, passed, "error_json").await, "null");
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_invariants_hold_for_task_list_and_task_cancel() {
    let harness = boot().await;
    harness
        .assert_reseller_sees_only_subtree("task.list", &JobFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row("task.cancel", &JobFixture)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn the_task_descriptors_match_their_snapshots_and_the_menu_carries_the_node() {
    let harness = Harness::new().module(&MODULE).boot().await;
    assert_eq!(harness.modules(), ["keel", "task", "testkit"]);
    for level in Level::ALL {
        let names = harness.snapshot_descriptors(level, snapshots_dir());
        let tasks: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| name.starts_with("task."))
            .collect();
        if level < Level::User {
            assert!(tasks.is_empty(), "{level}: {tasks:?}");
        } else {
            let mut expected = keel_system::tasks::FUNCTIONS.to_vec();
            expected.sort_unstable();
            assert_eq!(tasks, expected, "{level}");
        }
    }
    let http = harness.http().await;
    let booted = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(booted.status, 200, "{}", booted.body);
    let system = booted.json()["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .find(|group| group["id"] == json!("system"))
        .cloned()
        .expect("the system group");
    let node = system["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["func"] == json!("task.list"))
        .cloned()
        .unwrap_or_else(|| panic!("no tasks node in {system}"));
    assert_eq!(node["path"], json!(keel_system::tasks::MENU_PATH));
    assert_eq!(node["label"], json!("task.list.title"));
    assert_eq!(node["kind"], json!("list"));
    let guest = http.get(Who::Guest, "/api/v1/boot").await;
    assert!(
        !guest.body.contains("task.list"),
        "a guest sees the tasks node: {}",
        guest.body
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
