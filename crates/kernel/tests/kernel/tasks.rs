use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_kernel::dispatcher::{BATCH_LOG_PREFIX, CallOptions, Dispatcher, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, CallerOptions, TaskRunner, TaskSink, TaskSpec};
use keel_sdk::types::TaskState;
use keel_sdk::{Registrar, TaskHandle, migrator};
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Scope, Via};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

const ROOT: &str = "00000000000000000000000000";
const RESELLER: &str = "01J8A0000000000000000000B1";
const CUSTOMER: &str = "01J8A0000000000000000000B2";
const OTHER: &str = "01J8A0000000000000000000B3";

mod item {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "rack_item")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
        pub done: bool,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::AccountId
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct ItemEdit {
    title: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Run {
    label: String,
    #[serde(default)]
    enriched: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Ran {
    label: String,
    enriched: bool,
    task: Option<Id>,
    via: Via,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Sealed {
    label: String,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    password: Secret,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Batch {
    ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Finished {
    finished: u64,
    task: Option<Id>,
}

#[derive(Default)]
struct RunnerDouble {
    spawned: Mutex<Vec<(Id, TaskSpec)>>,
}

impl RunnerDouble {
    fn spawned_for(&self, principal: &Principal) -> Vec<(Id, TaskSpec)> {
        self.spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, spec)| spec.principal.id == principal.id)
            .cloned()
            .collect()
    }
}

#[derive(Default)]
struct Seen(Mutex<Vec<Id>>);

impl Seen {
    fn record(&self, principal: &Principal) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(principal.id);
    }

    fn count(&self, principal: &Principal) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|id| **id == principal.id)
            .count()
    }
}

#[async_trait]
impl TaskRunner for RunnerDouble {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id> {
        let id = Id::generate();
        self.spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, spec));
        Ok(id)
    }

    async fn progress(&self, _: Id, _: u64, _: Option<u64>, _: Option<&str>) -> Result<()> {
        Ok(())
    }

    async fn log(&self, _: Id, _: &str) -> Result<()> {
        Ok(())
    }

    async fn checkpoint(&self, _: Id, _: Value) -> Result<()> {
        Ok(())
    }

    async fn cancel(&self, _: Id) -> Result<()> {
        Ok(())
    }

    async fn state(&self, _: Id) -> Result<TaskState> {
        Ok(TaskState::Queued)
    }
}

struct SinkDouble {
    id: Id,
    checkpoint: Mutex<Option<Value>>,
    seen: Mutex<Vec<String>>,
}

impl SinkDouble {
    fn new(id: Id) -> Arc<Self> {
        Arc::new(Self {
            id,
            checkpoint: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl TaskSink for SinkDouble {
    fn id(&self) -> Id {
        self.id
    }

    async fn progress(&self, done: u64, total: Option<u64>, message: Option<&str>) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("progress {done}/{total:?} {message:?}"));
        Ok(())
    }

    async fn log(&self, line: &str) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("log {line}"));
        Ok(())
    }

    async fn checkpoint(&self, state: Value) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("checkpoint {state}"));
        *self
            .checkpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(state);
        Ok(())
    }

    fn resume_from(&self) -> Option<Value> {
        self.checkpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn output_path(&self) -> PathBuf {
        PathBuf::from("/nonexistent")
    }

    fn stored(&self, file: DownloadFile) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("stored {} {}", file.filename, file.content_type));
    }
}

static RUNNER: LazyLock<Arc<RunnerDouble>> = LazyLock::new(Arc::default);
static RUNS: LazyLock<Seen> = LazyLock::new(Seen::default);
static WITHIN: LazyLock<Seen> = LazyLock::new(Seen::default);

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/kernel/modules/rack/migrations")
        .table("keel_migration_rack");

static RACK: ModuleDef = ModuleDef {
    id: "rack",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

static LONE: ModuleDef = ModuleDef {
    id: "lone",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: lone_init,
};

static SEALED: ModuleDef = ModuleDef {
    id: "sealed",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: sealed_init,
};

const EN: &str = r#"{ "rack": { "item": {
    "edit": { "title": "Item" }, "run": { "title": "Run the rack" },
    "title": "Title", "account": "Account", "label": "Label", "enriched": "Enriched" } } }"#;
const LONE_EN: &str = r#"{ "lone": { "item": { "run": { "title": "Run alone" }, "label": "Label", "enriched": "Enriched" } } }"#;
const SEALED_EN: &str = r#"{ "sealed": { "item": { "run": { "title": "Run sealed" }, "label": "Label", "password": "Password" } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    r.function(edit())
        .function(run("rack.item.run"))
        .function(finish())
        .permission("rack.item.manage", &[Level::User, Level::Reseller])
        .i18n("en", EN)
        .provide::<dyn TaskRunner>(RUNNER.clone())
        .before("rack.item.run", |_: Call<Value>| async move {
            Ok(Before::Enrich(json!({ "enriched": true })))
        })
        .within("rack.item.run", |call: Call<Value>| async move {
            call.tx()?;
            WITHIN.record(call.principal());
            if call.params()["label"] == json!("veto") {
                return Err(KeelError::conflict("rack.item.error.veto").field("label"));
            }
            Ok(())
        });
    Ok(())
}

fn lone_init(r: &mut Registrar) -> Result<()> {
    r.function(run("lone.item.run")).i18n("en", LONE_EN);
    Ok(())
}

fn sealed_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Sealed, Ran>("sealed.item.run")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .ui(ActionScreen::new("sealed.item.run.title").result(ScreenResult::Task))
            .handler(|call: Call<Sealed>| async move {
                Ok(Ran {
                    label: call.params().label.clone(),
                    enriched: false,
                    task: None,
                    via: call.principal().via.clone(),
                })
            }),
    )
    .i18n("en", SEALED_EN);
    Ok(())
}

fn edit() -> FunctionDef {
    function::<ItemEdit, item::Model>("rack.item.edit")
        .kind(Kind::Form)
        .access(Level::User, "rack.item.manage")
        .unowned()
        .handler(|call: Call<ItemEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            Ok(item::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                title: Set(call.params().title.clone()),
                done: Set(false),
            }
            .insert(call.tx()?)
            .await?)
        })
}

fn run(name: &str) -> FunctionDef {
    function::<Run, Ran>(name)
        .kind(Kind::Action)
        .access(Level::User, "rack.item.manage")
        .unowned()
        .class("heavy")
        .resumable()
        .ui(ActionScreen::new(&format!("{name}.title")).result(ScreenResult::Task))
        .handler(|call: Call<Run>| async move {
            RUNS.record(call.principal());
            if let Some(task) = call.task() {
                task.progress(1, Some(1), Some("ran")).await?;
                task.log(&format!("ran {}", call.params().label)).await?;
            }
            Ok(Ran {
                label: call.params().label.clone(),
                enriched: call.params().enriched,
                task: call.task().map(TaskHandle::id),
                via: call.principal().via.clone(),
            })
        })
}

fn finish() -> FunctionDef {
    function::<Batch, Finished>("rack.item.finish")
        .kind(Kind::Action)
        .access(Level::User, "rack.item.manage")
        .key("ids")
        .owned::<item::Entity>()
        .batch_threshold(3)
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let finished = item::Entity::update_many()
                .col_expr(item::Column::Done, true.into())
                .filter(item::Column::Id.is_in(ids))
                .exec(call.tx()?)
                .await?
                .rows_affected;
            Ok(Finished {
                finished,
                task: call.task().map(TaskHandle::id),
            })
        })
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
}

fn principal(level: Level, account: &str) -> Principal {
    Principal {
        id: Id::generate(),
        login: format!("{level}-{account}"),
        level,
        account_id: account.parse().expect("account id"),
        role: "owner".into(),
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: "en".into(),
        grants: BTreeMap::new(),
    }
}

async fn seeded(composition: Composition) -> (TestDatabase, Storage, Kernel) {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(composition, &storage)
        .await
        .expect("the kernel boots");
    for (id, kind, parent, path) in [
        (RESELLER, "reseller", ROOT, format!("/{RESELLER}/")),
        (
            CUSTOMER,
            "user",
            RESELLER,
            format!("/{RESELLER}/{CUSTOMER}/"),
        ),
        (OTHER, "user", ROOT, format!("/{OTHER}/")),
    ] {
        storage
            .db()
            .execute_unprepared(&format!(
                "INSERT INTO keel_account (id, kind, parent_id, path, created_at, updated_at) VALUES ('{id}', '{kind}', '{parent}', '{path}', now(), now())"
            ))
            .await
            .expect("seed an account");
    }
    (database, storage, kernel)
}

async fn bench() -> Bench {
    let (database, storage, kernel) = seeded(Composition::new().module(&RACK)).await;
    let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
        .expect("every schema compiles");
    assert!(dispatcher.fill_caller());
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
    }
}

impl Bench {
    async fn call(&self, principal: &Principal, func: &str, params: Value) -> Reply {
        self.call_with(principal, func, params, CallOptions::default())
            .await
    }

    async fn call_with(
        &self,
        principal: &Principal,
        func: &str,
        params: Value,
        options: CallOptions,
    ) -> Reply {
        self.dispatcher
            .call(principal.clone(), func, params, options)
            .await
    }

    async fn item(&self, account: &str, title: &str) -> Id {
        let reply = self
            .call(
                &principal(Level::Owner, ROOT),
                "rack.item.edit",
                json!({ "title": title, "account": account }),
            )
            .await;
        reply
            .data()
            .unwrap_or_else(|| panic!("the item saves: {:?}", reply.error()))["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("ulid")
    }

    async fn done(&self) -> Vec<String> {
        let mut titles: Vec<String> = item::Entity::find()
            .filter(item::Column::Done.eq(true))
            .all(self.storage.db())
            .await
            .expect("items")
            .into_iter()
            .map(|row| row.title)
            .collect();
        titles.sort();
        titles
    }

    async fn run_job(&self, id: Id, spec: &TaskSpec) -> (Result<Value>, Arc<SinkDouble>) {
        let caller = self
            .kernel
            .registry()
            .services()
            .get::<dyn Caller>()
            .expect("the kernel provides the caller");
        let sink = SinkDouble::new(id);
        let outcome = caller
            .call(
                spec.principal.clone(),
                &spec.function,
                spec.params.clone(),
                CallerOptions {
                    request_id: Some(format!("task-{id}")),
                    task: Some(TaskHandle::new(sink.clone())),
                },
            )
            .await;
        (outcome, sink)
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn task_id(reply: &Reply) -> Id {
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("a task ref was expected, got {:?}", reply.error()));
    assert_eq!(
        data.as_object().map(|object| object.len()),
        Some(1),
        "{data}"
    );
    data["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid")
}

fn code(reply: &Reply) -> ErrorCode {
    reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()))
        .code
}

#[tokio::test]
async fn a_ui_result_task_function_answers_a_task_ref_and_its_handler_runs_in_the_worker() {
    let bench = bench().await;
    let caller = principal(Level::User, CUSTOMER);
    let reply = bench
        .call_with(
            &caller,
            "rack.item.run",
            json!({ "label": "first" }),
            CallOptions {
                request_id: Some("req-run".to_owned()),
                ..CallOptions::default()
            },
        )
        .await;
    let id = task_id(&reply);
    assert_eq!(RUNS.count(&caller), 0, "the handler did not run inline");
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let (spawned_id, spec) = &spawned[0];
    assert_eq!(*spawned_id, id);
    assert_eq!(spec.function, "rack.item.run");
    assert_eq!(spec.params, json!({ "label": "first", "enriched": true }));
    assert_eq!(spec.principal.id, caller.id);
    assert_eq!(spec.principal.account_id, caller.account_id);
    assert_eq!(spec.class, "heavy");
    assert!(spec.resumable);
    assert_eq!(spec.request_id.as_deref(), Some("req-run"));

    let (outcome, sink) = bench.run_job(id, spec).await;
    let ran = outcome.expect("the job runs the handler");
    assert_eq!(ran["label"], json!("first"));
    assert_eq!(ran["enriched"], json!(true));
    assert_eq!(ran["task"], json!(id.to_string()));
    assert_eq!(ran["via"], json!("internal"));
    assert_eq!(RUNS.count(&caller), 1);
    assert_eq!(
        sink.seen(),
        ["progress 1/Some(1) Some(\"ran\")", "log ran first"]
    );
    bench.finish().await;
}

#[tokio::test]
async fn a_job_run_by_the_worker_does_not_spawn_a_second_job() {
    let bench = bench().await;
    let caller = principal(Level::User, CUSTOMER);
    let reply = bench
        .call(&caller, "rack.item.run", json!({ "label": "once" }))
        .await;
    let id = task_id(&reply);
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let (outcome, _) = bench.run_job(id, &spawned[0].1).await;
    outcome.expect("the job runs");
    let (again, _) = bench.run_job(id, &spawned[0].1).await;
    again.expect("a resumed job runs the handler again without spawning");
    assert_eq!(RUNS.count(&caller), 2);
    assert_eq!(
        RUNNER.spawned_for(&caller).len(),
        1,
        "no second job was spawned"
    );
    bench.finish().await;
}

#[tokio::test]
async fn a_within_hook_vetoes_inside_the_job_and_an_idempotency_key_replays_the_task_ref() {
    let bench = bench().await;
    let caller = principal(Level::User, CUSTOMER);
    let reply = bench
        .call(&caller, "rack.item.run", json!({ "label": "veto" }))
        .await;
    let vetoed = task_id(&reply);
    assert_eq!(
        WITHIN.count(&caller),
        0,
        "the entrance leaves the within hooks to the job"
    );
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let (outcome, sink) = bench.run_job(vetoed, &spawned[0].1).await;
    let error = outcome.expect_err("the job raises the veto");
    assert_eq!(error.code(), ErrorCode::Validation);
    let wire = error.to_call_error();
    assert_eq!(wire.fields.len(), 1);
    assert_eq!(wire.fields[0].path, "label");
    assert_eq!(wire.fields[0].key.as_deref(), Some("rack.item.error.veto"));
    assert_eq!(WITHIN.count(&caller), 1);
    assert_eq!(RUNS.count(&caller), 0, "the veto stops the handler");
    assert!(sink.seen().is_empty(), "the handler wrote nothing");

    let keyed = |key: &str| CallOptions {
        idempotency_key: Some(key.to_owned()),
        ..CallOptions::default()
    };
    let first = bench
        .call_with(
            &caller,
            "rack.item.run",
            json!({ "label": "keyed" }),
            keyed("k-1"),
        )
        .await;
    let id = task_id(&first);
    assert!(!first.replay);
    let replayed = bench
        .call_with(
            &caller,
            "rack.item.run",
            json!({ "label": "keyed" }),
            keyed("k-1"),
        )
        .await;
    assert!(replayed.replay);
    assert_eq!(task_id(&replayed), id);
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 2, "the replay spawned nothing");
    assert_eq!(WITHIN.count(&caller), 1);

    let (outcome, _) = bench.run_job(id, &spawned[1].1).await;
    outcome.expect("the job runs");
    assert_eq!(
        WITHIN.count(&caller),
        2,
        "the job runs the within hooks inside its transaction"
    );
    assert_eq!(RUNS.count(&caller), 1);
    bench.finish().await;
}

#[tokio::test]
async fn a_composition_with_a_ui_result_task_function_and_no_runner_fails_freeze_naming_the_function()
 {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&LONE), &storage)
        .await
        .expect_err("no module provides a task runner");
    let text = error.to_string();
    assert!(text.contains("module lone"), "{text}");
    assert!(text.contains("lone.item.run"), "{text}");
    assert!(text.contains("task runner"), "{text}");
    storage.close().await.expect("the pool closes");
    database.finish().await;
}

#[tokio::test]
async fn a_secret_field_on_a_ui_result_task_function_fails_freeze() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&RACK).module(&SEALED), &storage)
        .await
        .expect_err("a job would store the password");
    let text = error.to_string();
    assert!(text.contains("module sealed"), "{text}");
    assert!(text.contains("sealed.item.run"), "{text}");
    assert!(text.contains("secret field password"), "{text}");
    storage.close().await.expect("the pool closes");
    database.finish().await;
}

#[tokio::test]
async fn a_batch_above_the_threshold_becomes_a_job_of_chunks_and_one_at_the_threshold_runs_inline()
{
    let bench = bench().await;
    let caller = principal(Level::User, CUSTOMER);
    let mut ids = Vec::new();
    for index in 0..10 {
        ids.push(bench.item(CUSTOMER, &format!("item {index}")).await);
    }
    let inline = bench
        .call(&caller, "rack.item.finish", json!({ "ids": ids[..3] }))
        .await;
    assert_eq!(
        inline.data().expect("three ids run inline"),
        &json!({ "finished": 3, "task": null })
    );
    assert_eq!(bench.done().await, ["item 0", "item 1", "item 2"]);
    assert!(RUNNER.spawned_for(&caller).is_empty());

    let reply = bench
        .call(&caller, "rack.item.finish", json!({ "ids": ids[3..] }))
        .await;
    let id = task_id(&reply);
    assert_eq!(bench.done().await.len(), 3, "the batch did not run inline");
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let spec = &spawned[0].1;
    assert_eq!(spec.function, "rack.item.finish");
    assert_eq!(spec.params, json!({ "ids": ids[3..] }));
    assert_eq!(spec.class, "default");
    assert!(!spec.resumable);

    let (outcome, sink) = bench.run_job(id, spec).await;
    let results = outcome.expect("the chunks run");
    assert_eq!(
        results,
        json!([
            { "finished": 3, "task": id.to_string() },
            { "finished": 3, "task": id.to_string() },
            { "finished": 1, "task": id.to_string() }
        ])
    );
    assert_eq!(bench.done().await.len(), 10);
    let seen = sink.seen();
    let progress: Vec<&String> = seen
        .iter()
        .filter(|line| line.starts_with("progress"))
        .collect();
    assert_eq!(
        progress,
        [
            "progress 3/Some(7) None",
            "progress 6/Some(7) None",
            "progress 7/Some(7) None"
        ]
    );
    let logs: Vec<&String> = seen.iter().filter(|line| line.starts_with("log")).collect();
    assert_eq!(
        logs,
        [
            &format!("log {BATCH_LOG_PREFIX} 1/3: 3 ids"),
            &format!("log {BATCH_LOG_PREFIX} 2/3: 3 ids"),
            &format!("log {BATCH_LOG_PREFIX} 3/3: 1 ids")
        ]
    );
    let checkpoints = seen
        .iter()
        .filter(|line| line.starts_with("checkpoint"))
        .count();
    assert_eq!(checkpoints, 3);
    assert_eq!(
        sink.resume_from().expect("the last checkpoint")["done"],
        json!(3)
    );
    assert_eq!(
        RUNNER.spawned_for(&caller).len(),
        1,
        "the chunks spawned nothing"
    );
    bench.finish().await;
}

#[tokio::test]
async fn a_batch_with_one_foreign_id_is_not_found_before_any_job_is_spawned() {
    let bench = bench().await;
    let caller = principal(Level::User, CUSTOMER);
    let mut ids = Vec::new();
    for index in 0..4 {
        ids.push(bench.item(CUSTOMER, &format!("mine {index}")).await);
    }
    ids.push(bench.item(OTHER, "theirs").await);
    let reply = bench
        .call(&caller, "rack.item.finish", json!({ "ids": ids }))
        .await;
    assert_eq!(code(&reply), ErrorCode::NotFound);
    assert!(bench.done().await.is_empty());
    assert!(RUNNER.spawned_for(&caller).is_empty());
    bench.finish().await;
}
