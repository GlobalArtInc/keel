use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use keel_kernel::dispatcher::{Dispatcher, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, CallerOptions, TaskRunner, TaskSink, TaskSpec};
use keel_sdk::types::TaskState;
use keel_sdk::{Registrar, TaskHandle};
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Scope, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const ROOT: &str = "00000000000000000000000000";

type Seen = Mutex<Vec<(Id, String)>>;

static SEEN: LazyLock<Seen> = LazyLock::new(Seen::default);
static SPAWNED: LazyLock<Mutex<Vec<TaskSpec>>> = LazyLock::new(Mutex::default);

fn seen(account: Id) -> Vec<String> {
    SEEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(of, _)| *of == account)
        .map(|(_, line)| line.clone())
        .collect()
}

fn spawned_for(account: Id) -> Vec<TaskSpec> {
    SPAWNED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|spec| spec.params["id"] == json!(account.to_string()))
        .cloned()
        .collect()
}

async fn record(module: &str, change: &str, call: Call<Id>) -> Result<()> {
    let account = *call.params();
    let row = call
        .tx()?
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM keel_account WHERE id = $1",
            [account.to_string().into()],
        ))
        .await?;
    let state = match row {
        Some(row) => row.try_get::<String>("", "state")?,
        None => "gone".to_owned(),
    };
    SEEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((account, format!("{module} {change} {state}")));
    Ok(())
}

static BASE: ModuleDef = ModuleDef {
    id: "base",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: init_base,
};

static UPPER: ModuleDef = ModuleDef {
    id: "upper",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["base"],
    migrations: None,
    init: init_upper,
};

static PLAIN: ModuleDef = ModuleDef {
    id: "plain",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["base"],
    migrations: None,
    init: init_plain,
};

static BREAKS: ModuleDef = ModuleDef {
    id: "breaks",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["base"],
    migrations: None,
    init: init_breaks,
};

fn init_base(r: &mut Registrar) -> Result<()> {
    r.contribute(
        AccountLifecycle::new()
            .on_suspend(|call| record("base", "suspend", call))
            .on_resume(|call| record("base", "resume", call))
            .on_delete(|call| record("base", "delete", call)),
    )
    .provide::<dyn TaskRunner>(Arc::new(RunnerDouble));
    Ok(())
}

fn init_upper(r: &mut Registrar) -> Result<()> {
    r.contribute(
        AccountLifecycle::new()
            .on_suspend(|call| async move {
                record("upper", "suspend", call).await?;
                Err(KeelError::internal("upper.error.after_commit"))
            })
            .on_resume(|call| record("upper", "resume", call))
            .on_delete(|call| record("upper", "delete", call))
            .on_rename(|call| record("upper", "rename", call)),
    );
    Ok(())
}

fn init_plain(_: &mut Registrar) -> Result<()> {
    Ok(())
}

fn init_breaks(r: &mut Registrar) -> Result<()> {
    r.contribute(AccountLifecycle::new().on_delete(|call| async move {
        record("breaks", "delete", call).await?;
        Err(KeelError::conflict("breaks.error.refused"))
    }));
    Ok(())
}

struct RunnerDouble;

#[async_trait]
impl TaskRunner for RunnerDouble {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id> {
        SPAWNED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(spec);
        Ok(Id::generate())
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

    async fn quiesce(&self, _: Id) -> Result<Vec<Id>> {
        Ok(Vec::new())
    }

    fn resume(&self) -> Result<()> {
        Ok(())
    }
}

struct SinkDouble {
    id: Id,
    checkpoint: Mutex<Option<Value>>,
    seen: Mutex<Vec<String>>,
}

impl SinkDouble {
    fn new(checkpoint: Option<Value>) -> Arc<Self> {
        Arc::new(Self {
            id: Id::generate(),
            checkpoint: Mutex::new(checkpoint),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn push(&self, line: String) {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
    }
}

#[async_trait]
impl TaskSink for SinkDouble {
    fn id(&self) -> Id {
        self.id
    }

    async fn progress(&self, done: u64, total: Option<u64>, message: Option<&str>) -> Result<()> {
        self.push(format!("progress {done}/{total:?} {message:?}"));
        Ok(())
    }

    async fn log(&self, line: &str) -> Result<()> {
        self.push(format!("log {line}"));
        Ok(())
    }

    async fn checkpoint(&self, state: Value) -> Result<()> {
        self.push(format!("checkpoint {state}"));
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

    fn stored(&self, _: DownloadFile) {}
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

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
    reseller: Id,
    customer: Id,
}

impl Bench {
    async fn boot(composition: Composition) -> Self {
        let database = TestDatabase::create().await;
        let storage = database.storage().await;
        let kernel = Kernel::boot(composition, &storage)
            .await
            .expect("the kernel boots");
        let reseller = Id::generate();
        let customer = Id::generate();
        let bench = Self {
            dispatcher: Dispatcher::new(kernel.registry().clone(), storage.db().clone())
                .expect("every schema compiles"),
            database,
            storage,
            kernel,
            reseller,
            customer,
        };
        assert!(bench.dispatcher.fill_caller());
        for sql in [
            format!(
                "INSERT INTO keel_account (id, kind, parent_id, path, created_at, updated_at) VALUES ('{reseller}', 'reseller', '{ROOT}', '/{reseller}/', now(), now())"
            ),
            format!(
                "INSERT INTO keel_account (id, kind, parent_id, path, created_at, updated_at) VALUES ('{customer}', 'user', '{reseller}', '/{reseller}/{customer}/', now(), now())"
            ),
            format!(
                "INSERT INTO keel_principal (id, account_id, login, level, created_at, updated_at) VALUES ('{}', '{customer}', 'customer-{customer}', 'user', now(), now())",
                Id::generate()
            ),
            format!(
                "INSERT INTO keel_limit (account_id, key, value) VALUES ('{customer}', 'base.items', '5')"
            ),
        ] {
            bench
                .storage
                .db()
                .execute_unprepared(&sql)
                .await
                .expect("seed the accounts");
        }
        bench
    }

    async fn call(&self, principal: &Principal, func: &str, params: Value) -> Reply {
        self.dispatcher
            .call(principal.clone(), func, params, Default::default())
            .await
    }

    async fn run_job(&self, spec: &TaskSpec, sink: Arc<SinkDouble>) -> Result<Value> {
        let caller = self
            .kernel
            .registry()
            .services()
            .get::<dyn Caller>()
            .expect("the kernel provides the caller");
        caller
            .call(
                spec.principal.clone(),
                &spec.function,
                spec.params.clone(),
                CallerOptions {
                    request_id: Some(format!("task-{}", sink.id)),
                    task: Some(TaskHandle::new(sink)),
                },
            )
            .await
    }

    async fn scalar(&self, sql: &str) -> Option<String> {
        self.storage
            .db()
            .query_one(Statement::from_string(DbBackend::Postgres, sql))
            .await
            .expect("the query runs")
            .map(|row| row.try_get_by_index::<String>(0).expect("a text column"))
    }

    async fn state(&self, account: Id) -> Option<String> {
        self.scalar(&format!(
            "SELECT state FROM keel_account WHERE id = '{account}'"
        ))
        .await
    }

    async fn events(&self, account: Id) -> Vec<String> {
        let rows = self
            .storage
            .db()
            .query_all(Statement::from_string(
                DbBackend::Postgres,
                format!(
                    "SELECT type FROM keel_outbox WHERE subject_json->>'id' = '{account}' ORDER BY seq"
                ),
            ))
            .await
            .expect("the outbox reads");
        rows.iter()
            .map(|row| row.try_get_by_index::<String>(0).expect("type"))
            .collect()
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

async fn settled(account: Id, count: usize) -> Vec<String> {
    for _ in 0..100 {
        let lines = seen(account);
        if lines.len() >= count {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    seen(account)
}

fn refusal(reply: &Reply) -> (ErrorCode, String) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("a refusal was expected, got {:?}", reply.data()));
    (error.code, error.key.clone())
}

#[tokio::test]
async fn suspend_and_resume_switch_the_state_and_walk_the_contributions_after_commit() {
    let bench = Bench::boot(
        Composition::new()
            .module(&BASE)
            .module(&UPPER)
            .module(&PLAIN),
    )
    .await;
    let owner = principal(Level::Owner, ROOT);
    let customer = bench.customer;

    let suspended = bench
        .call(&owner, "keel.account.suspend", json!({ "id": customer }))
        .await;
    assert_eq!(
        suspended.data(),
        Some(&json!({ "id": customer.to_string(), "state": "suspended", "changed": true })),
        "{:?}",
        suspended.error()
    );
    assert_eq!(bench.state(customer).await.as_deref(), Some("suspended"));
    assert_eq!(
        settled(customer, 2).await,
        ["base suspend suspended", "upper suspend suspended"]
    );
    assert_eq!(
        bench.state(customer).await.as_deref(),
        Some("suspended"),
        "the failing on_suspend of upper ran after the commit and undid nothing"
    );

    let again = bench
        .call(&owner, "keel.account.suspend", json!({ "id": customer }))
        .await;
    assert_eq!(
        again.data().map(|data| &data["changed"]),
        Some(&json!(false))
    );

    let resumed = bench
        .call(&owner, "keel.account.resume", json!({ "id": customer }))
        .await;
    assert_eq!(
        resumed.data(),
        Some(&json!({ "id": customer.to_string(), "state": "active", "changed": true })),
        "{:?}",
        resumed.error()
    );
    assert_eq!(
        settled(customer, 4).await,
        [
            "base suspend suspended",
            "upper suspend suspended",
            "base resume active",
            "upper resume active",
        ]
    );
    assert_eq!(
        bench.events(customer).await,
        ["keel.account.suspended", "keel.account.resumed"]
    );

    let root = bench
        .call(&owner, "keel.account.suspend", json!({ "id": ROOT }))
        .await;
    assert_eq!(
        refusal(&root),
        (
            ErrorCode::Conflict,
            "keel.account.error.root_immutable".to_owned()
        )
    );
    let own = bench
        .call(
            &principal(Level::Reseller, &bench.reseller.to_string()),
            "keel.account.suspend",
            json!({ "id": bench.reseller }),
        )
        .await;
    assert_eq!(
        refusal(&own),
        (
            ErrorCode::Conflict,
            "keel.account.error.own_account".to_owned()
        )
    );
    let user = bench
        .call(
            &principal(Level::User, &customer.to_string()),
            "keel.account.resume",
            json!({ "id": customer }),
        )
        .await;
    assert_eq!(refusal(&user).0, ErrorCode::Forbidden);
    assert!(
        !seen(customer).iter().any(|line| line.contains("rename")),
        "no change of this step renames an account"
    );
    bench.finish().await;
}

#[tokio::test]
async fn delete_runs_as_a_task_walking_on_delete_in_requires_order_and_removes_the_row_last() {
    let bench = Bench::boot(
        Composition::new()
            .module(&BASE)
            .module(&UPPER)
            .module(&PLAIN),
    )
    .await;
    let owner = principal(Level::Owner, ROOT);
    let customer = bench.customer;

    let parent = bench
        .call(
            &owner,
            "keel.account.delete",
            json!({ "id": bench.reseller }),
        )
        .await;
    assert_eq!(
        refusal(&parent),
        (
            ErrorCode::Conflict,
            "keel.account.error.has_children".to_owned()
        )
    );
    assert_eq!(bench.state(bench.reseller).await.as_deref(), Some("active"));

    let queued = bench
        .call(&owner, "keel.account.delete", json!({ "id": customer }))
        .await;
    let task = queued
        .data()
        .unwrap_or_else(|| panic!("a task ref was expected, got {:?}", queued.error()));
    assert!(task["task"]["id"].is_string(), "{task}");
    assert_eq!(bench.state(customer).await.as_deref(), Some("deleting"));
    assert!(seen(customer).is_empty(), "no handler runs before the job");
    let specs = spawned_for(customer);
    assert_eq!(specs.len(), 1);
    let spec = &specs[0];
    assert_eq!(spec.function, "keel.account.delete");
    assert!(spec.resumable);
    assert_eq!(spec.principal.id, owner.id);

    let sink = SinkDouble::new(None);
    let result = bench
        .run_job(spec, sink.clone())
        .await
        .expect("the job deletes the account");
    assert_eq!(
        result,
        json!({ "id": customer.to_string(), "modules": ["upper", "base"] })
    );
    assert_eq!(
        seen(customer),
        ["upper delete deleting", "base delete deleting"]
    );
    let modules = Some("keel.account.delete.progress.modules");
    assert_eq!(
        sink.seen(),
        [
            format!("log deleting account {customer} for {}", owner.login),
            format!("progress 0/Some(3) {modules:?}"),
            r#"checkpoint {"done":["upper"]}"#.to_owned(),
            "log deleted upper".to_owned(),
            format!("progress 1/Some(3) {modules:?}"),
            r#"checkpoint {"done":["upper","base"]}"#.to_owned(),
            "log deleted base".to_owned(),
            format!(
                "progress 2/Some(3) {:?}",
                Some("keel.account.delete.progress.row")
            ),
            "log deleted the account row".to_owned(),
            "progress 3/Some(3) None".to_owned(),
        ]
    );
    assert_eq!(bench.state(customer).await, None);
    assert_eq!(
        bench
            .scalar(&format!(
                "SELECT count(*)::text FROM keel_principal WHERE account_id = '{customer}'"
            ))
            .await
            .as_deref(),
        Some("0")
    );
    assert_eq!(bench.events(customer).await, ["keel.account.deleted"]);
    assert_eq!(
        bench
            .scalar(&format!(
                "SELECT string_agg(type || ' ' || (subject_json->>'kind') || ' ' || (subject_json->>'id' = data_json->>'id')::text, ',' ORDER BY seq) FROM keel_outbox WHERE subject_json->>'accountId' = '{customer}'"
            ))
            .await
            .as_deref(),
        Some("keel.principal.deleted keel.principal true,keel.account.deleted keel.account true")
    );
    assert_eq!(bench.state(bench.reseller).await.as_deref(), Some("active"));

    let last = Id::generate();
    bench
        .storage
        .db()
        .execute_unprepared(&format!(
            "INSERT INTO keel_account (id, kind, parent_id, path, state, created_at, updated_at) VALUES ('{last}', 'user', '{ROOT}', '/{last}/', 'deleting', now(), now())"
        ))
        .await
        .expect("seed an interrupted deletion");
    let resumed = TaskSpec {
        params: json!({ "id": last }),
        ..spec.clone()
    };
    let sink = SinkDouble::new(Some(json!({ "done": ["upper"] })));
    let result = bench
        .run_job(&resumed, sink)
        .await
        .expect("the resumed job finishes");
    assert_eq!(
        result,
        json!({ "id": last.to_string(), "modules": ["upper", "base"] })
    );
    assert_eq!(seen(last), ["base delete deleting"]);
    assert_eq!(bench.state(last).await, None);
    bench.finish().await;
}

#[tokio::test]
async fn a_failing_on_delete_leaves_the_account_deleting_with_the_error_on_the_task() {
    let bench = Bench::boot(Composition::new().module(&BASE).module(&BREAKS)).await;
    let owner = principal(Level::Owner, ROOT);
    let customer = bench.customer;
    let queued = bench
        .call(&owner, "keel.account.delete", json!({ "id": customer }))
        .await;
    assert!(queued.data().is_some(), "{:?}", queued.error());
    let specs = spawned_for(customer);
    assert_eq!(specs.len(), 1);

    let sink = SinkDouble::new(None);
    let error = bench
        .run_job(&specs[0], sink.clone())
        .await
        .expect_err("the job fails");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.key(), "breaks.error.refused");
    assert_eq!(error.params().get("module"), Some(&json!("breaks")));
    assert_eq!(seen(customer), ["breaks delete deleting"]);
    assert!(
        sink.seen()
            .iter()
            .all(|line| !line.starts_with("checkpoint")),
        "{:?}",
        sink.seen()
    );
    assert_eq!(bench.state(customer).await.as_deref(), Some("deleting"));
    assert_eq!(bench.events(customer).await, Vec::<String>::new());

    let suspend = bench
        .call(&owner, "keel.account.suspend", json!({ "id": customer }))
        .await;
    assert_eq!(
        refusal(&suspend),
        (
            ErrorCode::Conflict,
            "keel.account.error.deleting".to_owned()
        )
    );
    let again = bench
        .call(&owner, "keel.account.delete", json!({ "id": customer }))
        .await;
    assert!(again.data().is_some(), "{:?}", again.error());
    assert_eq!(
        spawned_for(customer).len(),
        2,
        "a failed deletion can be run again"
    );
    bench.finish().await;
}
