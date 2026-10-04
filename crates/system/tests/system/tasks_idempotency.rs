use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::ConnectionTrait;
use keel_sdk::types::{ErrorCode, TaskState};
use keel_sdk::{Registrar, serde_json};
use keel_system::tasks::MODULE;
use keel_system::tasks::api::Drain;
use keel_testkit::{CallOptions, Harness, Reply, Who, tokio};

const EXPORT: &str = "ledger.entry.export";
const DEFER: &str = "ledger.entry.defer";
const VETO: &str = "ledger.error.veto";
const CROWD: usize = 60;
const PATIENCE: Duration = Duration::from_secs(30);

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Entry {
    label: String,
    amount: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Exported {
    label: String,
}

static LEDGER: ModuleDef = ModuleDef {
    id: "ledger",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["task"],
    migrations: None,
    init,
};

const EN: &str = r#"{ "ledger": { "entry": {
    "export": { "title": "Export the ledger" }, "label": "Label", "amount": "Amount" } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Entry, Exported>(EXPORT)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .idempotent(Idempotency::Required)
            .ui(ActionScreen::new("ledger.entry.export.title").result(ScreenResult::Task))
            .handler(|call: Call<Entry>| async move {
                Ok(Exported {
                    label: call.params().label.clone(),
                })
            }),
    )
    .function(
        function::<Entry, Value>(DEFER)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Entry>| async move {
                if call.task().is_some() {
                    return Ok(json!({ "ran": call.params().label }));
                }
                let reference = call.spawn_self_as_task().await?;
                if call.params().label == "veto" {
                    return Err(KeelError::conflict(VETO));
                }
                Ok(serde_json::to_value(reference)?)
            }),
    )
    .i18n("en", EN);
    Ok(())
}

async fn boot() -> Harness {
    Harness::new().module(&MODULE).module(&LEDGER).boot().await
}

fn keyed(key: &str) -> CallOptions {
    CallOptions {
        idempotency_key: Some(key.to_owned()),
        ..CallOptions::default()
    }
}

fn task(reply: &Reply) -> Id {
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()));
    data["task"]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("a task reference was expected, got {data}"))
}

async fn jobs(harness: &Harness, func: &str) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT id FROM task_job WHERE function = '{func}' ORDER BY id"
        ))
        .await
}

async fn keys(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key FROM keel_idempotency ORDER BY key")
        .await
}

async fn settled(harness: &Harness, id: Id) -> TaskState {
    let drain = harness
        .registry()
        .services()
        .get::<Drain>()
        .expect("the tasks module provides the drain");
    tokio::time::timeout(PATIENCE, drain.wait(id))
        .await
        .expect("the job ends in time")
        .expect("the state is read")
}

#[tokio::test]
async fn a_keyed_spawn_whose_record_fails_leaves_no_job_and_its_retry_queues_one() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    harness
        .db()
        .execute_unprepared(
            "CREATE FUNCTION ledger_refuse() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
                 IF NEW.key = 'broken-1' THEN
                     RAISE EXCEPTION 'the record is refused';
                 END IF;
                 RETURN NEW;
             END $$;
             CREATE TRIGGER ledger_refuse BEFORE INSERT ON keel_idempotency
                 FOR EACH ROW EXECUTE FUNCTION ledger_refuse();",
        )
        .await
        .expect("the trigger is created");
    let body = json!({ "label": "broken", "amount": 1 });
    let call = || {
        harness
            .dispatcher()
            .call(a1.clone(), EXPORT, body.clone(), keyed("broken-1"))
    };
    let refused = call().await;
    assert_eq!(
        refused.error().map(|error| error.code),
        Some(ErrorCode::Internal),
        "{:?}",
        refused.data()
    );
    assert!(
        jobs(&harness, EXPORT).await.is_empty(),
        "the job went with the transaction of the refused record"
    );
    assert!(keys(&harness).await.is_empty());

    harness
        .db()
        .execute_unprepared("DROP TRIGGER ledger_refuse ON keel_idempotency")
        .await
        .expect("the trigger is dropped");
    let retried = call().await;
    let id = task(&retried);
    assert_eq!(jobs(&harness, EXPORT).await, [id.to_string()]);
    assert_eq!(settled(&harness, id).await, TaskState::Done);
    let again = call().await;
    assert!(again.replay);
    assert_eq!(task(&again), id);
    assert_eq!(
        jobs(&harness, EXPORT).await,
        [id.to_string()],
        "the key holds one job"
    );
    assert_eq!(keys(&harness).await, ["broken-1"]);
    harness.finish().await;
}

#[tokio::test]
async fn keyed_spawns_beyond_the_pool_size_all_answer_and_their_jobs_run() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let calls = (0..CROWD).map(|n| {
        harness.dispatcher().call(
            a1.clone(),
            EXPORT,
            json!({ "label": "crowd", "amount": n }),
            keyed(&format!("crowd-{n:02}")),
        )
    });
    let replies = tokio::time::timeout(PATIENCE, futures::future::join_all(calls))
        .await
        .expect("no keyed spawn waits for a second connection");
    let mut ids: Vec<Id> = replies.iter().map(task).collect();
    ids.sort();
    assert_eq!(
        jobs(&harness, EXPORT).await,
        ids.iter().map(Id::to_string).collect::<Vec<_>>()
    );
    assert_eq!(keys(&harness).await.len(), CROWD);
    for id in ids {
        assert_eq!(settled(&harness, id).await, TaskState::Done);
    }
    harness.finish().await;
}

#[tokio::test]
async fn a_job_a_write_call_queues_for_itself_commits_with_the_call_or_not_at_all() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let call = |label: &str, key: &str| {
        harness.dispatcher().call(
            a1.clone(),
            DEFER,
            json!({ "label": label, "amount": 2 }),
            keyed(key),
        )
    };
    let vetoed = call("veto", "veto-1").await;
    assert_eq!(vetoed.error().map(|error| error.key.as_str()), Some(VETO));
    assert!(
        jobs(&harness, DEFER).await.is_empty(),
        "the job went with the call's transaction"
    );
    assert!(keys(&harness).await.is_empty());

    let deferred = call("later", "later-1").await;
    let id = task(&deferred);
    assert_eq!(jobs(&harness, DEFER).await, [id.to_string()]);
    assert_eq!(settled(&harness, id).await, TaskState::Done);
    let again = call("later", "later-1").await;
    assert!(again.replay);
    assert_eq!(task(&again), id);
    assert_eq!(jobs(&harness, DEFER).await, [id.to_string()]);
    assert_eq!(keys(&harness).await, ["later-1"]);
    harness.finish().await;
}
