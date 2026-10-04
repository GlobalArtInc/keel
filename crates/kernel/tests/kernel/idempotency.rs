use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use keel_kernel::Composition;
use keel_kernel::ctl::{self, Out};
use keel_kernel::dispatcher::{
    CallOptions, IDEMPOTENCY_HOURS_SETTING, IDEMPOTENCY_KEY_MAX_LEN, IDEMPOTENCY_MALFORMED_KEY,
    IDEMPOTENCY_REQUIRED_KEY, Reply,
};
use keel_kernel::http::{IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY};
use keel_kernel::keel::IDEMPOTENCY_PRUNE;
use keel_kernel::socket::{Client, Socket};
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, CallerOptions, ContextExt, TaskRunner, TaskSpec};
use keel_sdk::{Registrar, migrator};
use keel_testkit::{Answer, Harness, PRINCIPAL_HEADER, TestDatabase, TestHttp, Who};
use keel_types::{Envelope, ErrorCode, TaskState};
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbBackend, Set, Statement};
use tokio::sync::Semaphore;

const ADD: &str = "ledger.entry.add";
const OPEN: &str = "ledger.entry.open";
const EXPORT: &str = "ledger.entry.export";
const QUEUE: &str = "ledger.entry.queue";
const DEFER: &str = "ledger.entry.defer";
const SUM: &str = "ledger.entry.sum";
const VETO: &str = "ledger.error.veto";
const START: &str = "2026-10-01T00:00:00Z";
const PATIENCE: Duration = Duration::from_secs(15);
const CUT: Duration = Duration::from_millis(300);
const CROWD: usize = 60;

mod entry {
    use keel_sdk::sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "ledger_entry")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub label: String,
        pub amount: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Add {
    label: String,
    amount: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Added {
    id: Id,
    label: String,
    amount: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Sum {
    total: i64,
}

static ENTERED: LazyLock<Mutex<BTreeMap<String, usize>>> = LazyLock::new(Mutex::default);
static GATES: LazyLock<Mutex<BTreeMap<String, Arc<Semaphore>>>> = LazyLock::new(Mutex::default);

fn entered(label: &str) -> usize {
    ENTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(label)
        .copied()
        .unwrap_or_default()
}

fn gate(label: &str) -> Arc<Semaphore> {
    let gate = Arc::new(Semaphore::new(0));
    GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(label.to_owned(), gate.clone());
    gate
}

async fn pass(label: &str) {
    let gate = GATES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(label)
        .cloned();
    if let Some(gate) = gate {
        let _ = gate.acquire().await;
    }
}

async fn add(call: Call<Add>) -> Result<Added> {
    let Add { label, amount } = call.params();
    *ENTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(label.clone())
        .or_default() += 1;
    pass(label).await;
    let id = Id::generate();
    entry::ActiveModel {
        id: Set(id.to_string()),
        account_id: Set(call.principal().account_id.to_string()),
        label: Set(label.clone()),
        amount: Set(*amount),
    }
    .insert(call.tx()?)
    .await?;
    call.emit("ledger.entry.added", &json!({ "id": id, "label": label }))?;
    Ok(Added {
        id,
        label: label.clone(),
        amount: *amount,
    })
}

async fn defer(call: Call<Add>) -> Result<TaskRef> {
    let reference = call.spawn_self_as_task().await?;
    if call.params().label == "veto" {
        return Err(KeelError::conflict(VETO));
    }
    Ok(reference)
}

#[derive(Default)]
struct Runner {
    pool: OnceLock<Db>,
}

impl Runner {
    fn pool(&self) -> Result<&Db> {
        self.pool
            .get()
            .ok_or_else(|| KeelError::unavailable("ledger.error.runner_idle"))
    }

    async fn queue(&self, db: &Db, spec: &TaskSpec) -> Result<Id> {
        let id = Id::generate();
        let label = spec.params["label"].as_str().unwrap_or_default().to_owned();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO ledger_job (id, label) VALUES ($1, $2)",
            [id.to_string().into(), label.clone().into()],
        ))
        .await?;
        *ENTERED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(label.clone())
            .or_default() += 1;
        pass(&label).await;
        Ok(id)
    }
}

#[async_trait]
impl TaskRunner for Runner {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id> {
        self.queue(self.pool()?, &spec).await
    }

    async fn enqueue(&self, db: &Db, spec: TaskSpec) -> Result<Id> {
        self.queue(db, &spec).await
    }

    async fn wake(&self, id: Id) -> Result<()> {
        self.pool()?
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE ledger_job SET woken = true WHERE id = $1",
                [id.to_string().into()],
            ))
            .await?;
        Ok(())
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

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/kernel/modules/ledger/migrations")
        .table("keel_migration_ledger");

static LEDGER: ModuleDef = ModuleDef {
    id: "ledger",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

const EN: &str = r#"{ "ledger": { "entry": {
    "export": { "title": "Export the ledger" }, "queue": { "title": "Queue the ledger" },
    "label": "Label", "amount": "Amount" } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    let runner = Arc::new(Runner::default());
    let started = runner.clone();
    r.function(
        function::<Add, Added>(ADD)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(add),
    )
    .function(
        function::<Add, Added>(OPEN)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .idempotent(Idempotency::Required)
            .handler(add),
    )
    .function(
        function::<Add, Added>(EXPORT)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .idempotent(Idempotency::Required)
            .ui(ActionScreen::new("ledger.entry.export.title").result(ScreenResult::Task))
            .handler(add),
    )
    .function(
        function::<Add, Added>(QUEUE)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .timeout(CUT)
            .ui(ActionScreen::new("ledger.entry.queue.title").result(ScreenResult::Task))
            .handler(add),
    )
    .function(
        function::<Add, TaskRef>(DEFER)
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(defer),
    )
    .function(
        function::<Nothing, Sum>(SUM)
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Sum { total: 0 }) }),
    )
    .i18n("en", EN)
    .provide::<dyn TaskRunner>(runner)
    .start(move |ctx| async move {
        let _ = started.pool.set(ctx.db());
        Ok(())
    });
    Ok(())
}

struct TwoHours;

#[async_trait]
impl SettingsSource for TwoHours {
    async fn resolve(&self, key: &str, account: Id, _: &Db) -> Result<Option<Value>> {
        assert_eq!(
            account,
            Id::ROOT_ACCOUNT,
            "the lifetime is the installation's"
        );
        Ok((key == IDEMPOTENCY_HOURS_SETTING).then(|| json!(2)))
    }
}

static SHORT: ModuleDef = ModuleDef {
    id: "short",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: short_init,
};

fn short_init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn SettingsSource>(Arc::new(TwoHours));
    Ok(())
}

static MISDECLARED: ModuleDef = ModuleDef {
    id: "misdeclared",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: misdeclared_init,
};

fn misdeclared_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Sum>("misdeclared.entry.sum")
            .kind(Kind::Get)
            .level(Level::User)
            .unowned()
            .idempotent(Idempotency::Required)
            .handler(|_: Call<Nothing>| async { Ok(Sum { total: 0 }) }),
    )
    .function(
        function::<Nothing, Sum>("misdeclared.entry.claim")
            .kind(Kind::Action)
            .level(Level::Guest)
            .unowned()
            .idempotent(Idempotency::Required)
            .handler(|_: Call<Nothing>| async { Ok(Sum { total: 0 }) }),
    )
    .function(
        function::<Nothing, Sum>("misdeclared.entry.open")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .idempotent(Idempotency::Required)
            .handler(|_: Call<Nothing>| async { Ok(Sum { total: 0 }) }),
    );
    Ok(())
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

async fn boot() -> Harness {
    Harness::new().module(&LEDGER).clock(at(START)).boot().await
}

fn keyed(key: &str) -> CallOptions {
    CallOptions {
        idempotency_key: Some(key.to_owned()),
        ..CallOptions::default()
    }
}

async fn post(http: &TestHttp, who: Who, func: &str, key: Option<&str>, body: &Value) -> Answer {
    let header = IDEMPOTENCY_KEY;
    let mut headers = vec![
        (PRINCIPAL_HEADER, who.login()),
        ("content-type", "application/json"),
    ];
    if let Some(key) = key {
        headers.push((header.as_str(), key));
    }
    http.request(
        "POST",
        &format!("/api/v1/call/{func}"),
        &headers,
        Some(&body.to_string()),
    )
    .await
}

fn ok(answer: &Answer) -> Value {
    let body = answer.json();
    assert_eq!(answer.status, 200, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");
    body["data"].clone()
}

fn refused(answer: &Answer, status: u16, code: ErrorCode, key: &str) -> Value {
    let body = answer.json();
    assert_eq!(answer.status, status, "{body}");
    assert_eq!(body["error"]["code"], json!(code), "{body}");
    assert_eq!(body["error"]["key"], json!(key), "{body}");
    assert_eq!(answer.header(IDEMPOTENT_REPLAY.as_str()), None);
    body["error"].clone()
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

async fn count(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

async fn entries(harness: &Harness, label: &str) -> usize {
    count(
        harness,
        &format!("SELECT id FROM ledger_entry WHERE label = '{label}'"),
    )
    .await
    .len()
}

async fn jobs(harness: &Harness, label: &str) -> Vec<String> {
    count(
        harness,
        &format!("SELECT id || ':' || woken FROM ledger_job WHERE label = '{label}' ORDER BY id"),
    )
    .await
}

fn woken(reply: &Reply) -> String {
    let id = data(reply)["task"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a task reference was expected, got {:?}", data(reply)))
        .to_owned();
    format!("{id}:true")
}

async fn keys(harness: &Harness) -> Vec<String> {
    count(
        harness,
        "SELECT key FROM keel_idempotency ORDER BY expires_at, key",
    )
    .await
}

async fn waiting(harness: &Harness) -> usize {
    count(
        harness,
        "SELECT count(*)::text FROM pg_locks WHERE locktype = 'advisory' AND NOT granted AND database = (SELECT oid FROM pg_database WHERE datname = current_database())",
    )
    .await
    .first()
    .and_then(|count| count.parse().ok())
    .unwrap_or_default()
}

async fn until(what: &str, mut check: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn prune(harness: &Harness) -> Value {
    let caller = harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    caller
        .call(
            harness.principal(Who::Owner).clone(),
            IDEMPOTENCY_PRUNE,
            json!({}),
            CallerOptions::default(),
        )
        .await
        .unwrap_or_else(|error| panic!("the prune runs: {error:?}"))
}

#[tokio::test]
async fn a_repeat_with_the_same_key_and_body_replays_the_response() {
    let harness = boot().await;
    let http = harness.http().await;
    let first = post(
        &http,
        Who::A1,
        ADD,
        Some("rent-1"),
        &json!({ "label": "rent", "amount": 10 }),
    )
    .await;
    let answered = ok(&first);
    assert_eq!(first.header(IDEMPOTENT_REPLAY.as_str()), None);
    assert_eq!(answered["label"], json!("rent"));

    let repeat = post(
        &http,
        Who::A1,
        ADD,
        Some("rent-1"),
        &json!({ "amount": 10, "label": "rent" }),
    )
    .await;
    assert_eq!(
        ok(&repeat),
        answered,
        "the stored response comes back whole"
    );
    assert_eq!(repeat.header(IDEMPOTENT_REPLAY.as_str()), Some("true"));
    assert_eq!(entered("rent"), 1, "the handler ran once");
    assert_eq!(entries(&harness, "rent").await, 1);
    assert_eq!(
        count(
            &harness,
            "SELECT type FROM keel_outbox WHERE data_json->>'label' = 'rent'"
        )
        .await,
        ["ledger.entry.added"],
        "a replay writes no second event"
    );
    assert_eq!(keys(&harness).await, ["rent-1"]);

    let other = post(
        &http,
        Who::A2,
        ADD,
        Some("rent-1"),
        &json!({ "label": "rent", "amount": 10 }),
    )
    .await;
    assert_ne!(ok(&other)["id"], answered["id"], "keys are per principal");
    assert_eq!(other.header(IDEMPOTENT_REPLAY.as_str()), None);
    assert_eq!(entries(&harness, "rent").await, 2);

    let dir = tempfile::tempdir().expect("a temporary directory");
    let serving = Socket::new(harness.dispatcher().clone())
        .start(&dir.path().join("core.sock"))
        .await
        .expect("the socket starts");
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    let mut framed = Vec::new();
    for body in [
        json!({ "label": "rent", "amount": 10 }),
        json!({ "amount": 10, "label": "rent" }),
    ] {
        match client
            .call(ADD, body, keyed("rent-1"))
            .await
            .expect("the socket answers")
        {
            Envelope::Ok { data, .. } => framed.push(data),
            Envelope::Err { error } => panic!("the frame is answered: {error:?}"),
        }
    }
    assert_eq!(framed[0], framed[1], "the frame's idempotencyKey replays");
    assert_ne!(framed[0]["id"], answered["id"]);
    assert_eq!(entries(&harness, "rent").await, 3);
    assert_eq!(entered("rent"), 3);
    serving.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_same_key_with_another_body_answers_mismatch() {
    let harness = boot().await;
    let http = harness.http().await;
    let body = json!({ "label": "fee", "amount": 5 });
    let first = post(&http, Who::A1, ADD, Some("fee-1"), &body).await;
    let answered = ok(&first);

    let changed = post(
        &http,
        Who::A1,
        ADD,
        Some("fee-1"),
        &json!({ "label": "fee", "amount": 6 }),
    )
    .await;
    refused(
        &changed,
        409,
        ErrorCode::IdempotencyMismatch,
        "keel.error.idempotency_mismatch",
    );
    let elsewhere = post(&http, Who::A1, OPEN, Some("fee-1"), &body).await;
    refused(
        &elsewhere,
        409,
        ErrorCode::IdempotencyMismatch,
        "keel.error.idempotency_mismatch",
    );
    assert_eq!(entered("fee"), 1, "neither refusal reached the handler");
    assert_eq!(entries(&harness, "fee").await, 1);
    assert_eq!(keys(&harness).await, ["fee-1"]);

    let repeat = post(&http, Who::A1, ADD, Some("fee-1"), &body).await;
    assert_eq!(
        ok(&repeat),
        answered,
        "the record outlives a refused repeat"
    );
    assert_eq!(repeat.header(IDEMPOTENT_REPLAY.as_str()), Some("true"));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_required_key_missing_answers_validation() {
    let harness = boot().await;
    let http = harness.http().await;
    let body = json!({ "label": "deposit", "amount": 100 });
    for key in [None, Some("   ")] {
        let missing = post(&http, Who::A1, OPEN, key, &body).await;
        let error = refused(
            &missing,
            422,
            ErrorCode::Validation,
            IDEMPOTENCY_REQUIRED_KEY,
        );
        assert_eq!(error["params"], json!({ "function": OPEN }));
        assert_eq!(error.get("fields"), None, "the key is no parameter");
    }
    let invalid = post(&http, Who::A1, OPEN, None, &json!({ "amount": 100 })).await;
    let error = refused(
        &invalid,
        422,
        ErrorCode::Validation,
        "keel.error.validation",
    );
    assert_eq!(
        error["fields"][0]["path"],
        json!("label"),
        "the parameters are validated before the key is asked for"
    );
    assert_eq!(entered("deposit"), 0);
    assert_eq!(entries(&harness, "deposit").await, 0);
    assert!(keys(&harness).await.is_empty());

    let first = post(&http, Who::A1, OPEN, Some("deposit-1"), &body).await;
    let opened = ok(&first);
    let again = post(&http, Who::A1, OPEN, Some("deposit-1"), &body).await;
    assert_eq!(ok(&again), opened);
    assert_eq!(again.header(IDEMPOTENT_REPLAY.as_str()), Some("true"));
    assert_eq!(entries(&harness, "deposit").await, 1);

    let dir = tempfile::tempdir().expect("a temporary directory");
    let serving = Socket::new(harness.dispatcher().clone())
        .start(&dir.path().join("core.sock"))
        .await
        .expect("the socket starts");
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    match client
        .call(OPEN, body.clone(), CallOptions::default())
        .await
        .expect("the socket answers")
    {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(error.key, IDEMPOTENCY_REQUIRED_KEY);
        }
        Envelope::Ok { data, .. } => panic!("a frame without a key ran: {data}"),
    }
    match client
        .call(OPEN, body.clone(), keyed("deposit-1"))
        .await
        .expect("the socket answers")
    {
        Envelope::Ok { data, .. } => assert_ne!(data["id"], opened["id"]),
        Envelope::Err { error } => panic!("a keyed frame is admitted: {error:?}"),
    }
    serving.stop().await.expect("the socket stops");
    assert_eq!(entries(&harness, "deposit").await, 2);

    let caller = harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    caller
        .call(
            harness.principal(Who::A1).clone(),
            OPEN,
            body.clone(),
            CallerOptions::default(),
        )
        .await
        .expect("an in-process call through the caller needs no key");
    assert_eq!(entries(&harness, "deposit").await, 3);

    let described = http
        .get(Who::A1, &format!("/api/v1/functions/{OPEN}"))
        .await
        .json();
    assert_eq!(described["idempotency"], json!("required"));
    let plain = http
        .get(Who::A1, &format!("/api/v1/functions/{ADD}"))
        .await
        .json();
    assert_eq!(plain["name"], json!(ADD));
    assert_eq!(plain.get("idempotency"), None);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn keys_expire_and_are_pruned() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let early = json!({ "label": "early", "amount": 1 });
    let late = json!({ "label": "late", "amount": 2 });
    let call = |body: &Value, key: &str| {
        harness
            .dispatcher()
            .call(a1.clone(), ADD, body.clone(), keyed(key))
    };
    let first = call(&early, "early-1").await;
    assert!(!first.replay);
    harness.clock().advance(chrono::Duration::hours(12));
    assert!(!call(&late, "late-1").await.replay);
    assert_eq!(
        count(
            &harness,
            "SELECT to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') FROM keel_idempotency ORDER BY expires_at"
        )
        .await,
        ["2026-10-02 00:00", "2026-10-02 12:00"],
        "a record lives 24 hours by default"
    );

    harness
        .clock()
        .advance(chrono::Duration::hours(12) - chrono::Duration::seconds(1));
    let live = call(&early, "early-1").await;
    assert!(live.replay, "a record replays until its expiry");
    assert_eq!(data(&live), data(&first));
    assert_eq!(prune(&harness).await, json!({ "pruned": 0 }));
    assert_eq!(keys(&harness).await, ["early-1", "late-1"]);

    harness.clock().advance(chrono::Duration::seconds(1));
    assert_eq!(prune(&harness).await, json!({ "pruned": 1 }));
    assert_eq!(
        keys(&harness).await,
        ["late-1"],
        "the expired record is gone and the live one stays"
    );
    assert!(call(&late, "late-1").await.replay);
    let lapsed = call(&early, "early-1").await;
    assert!(!lapsed.replay, "an expired key runs the call again");
    assert_ne!(data(&lapsed)["id"], data(&first)["id"]);
    assert_eq!(entries(&harness, "early").await, 2);
    assert_eq!(keys(&harness).await, ["late-1", "early-1"]);

    harness.clock().advance(chrono::Duration::hours(48));
    assert_eq!(prune(&harness).await, json!({ "pruned": 2 }));
    assert!(keys(&harness).await.is_empty());

    let http = harness.http().await;
    let outside = post(&http, Who::Owner, IDEMPOTENCY_PRUNE, None, &json!({})).await;
    refused(
        &outside,
        403,
        ErrorCode::Forbidden,
        "keel.error.internal_only",
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_lifetime_of_a_record_follows_the_hours_setting() {
    let harness = Harness::new()
        .module(&LEDGER)
        .module(&SHORT)
        .clock(at(START))
        .boot()
        .await;
    let a1 = harness.principal(Who::A1).clone();
    let body = json!({ "label": "short", "amount": 3 });
    let call = || {
        harness
            .dispatcher()
            .call(a1.clone(), ADD, body.clone(), keyed("short-1"))
    };
    assert!(!call().await.replay);
    assert_eq!(
        count(
            &harness,
            "SELECT to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') FROM keel_idempotency"
        )
        .await,
        ["2026-10-01 02:00"]
    );
    harness
        .clock()
        .advance(chrono::Duration::hours(2) - chrono::Duration::seconds(1));
    assert!(call().await.replay);
    harness.clock().advance(chrono::Duration::seconds(1));
    assert_eq!(prune(&harness).await, json!({ "pruned": 1 }));
    assert!(!call().await.replay);
    assert_eq!(entries(&harness, "short").await, 2);
    harness.finish().await;
}

#[tokio::test]
async fn a_concurrent_repeat_waits_for_the_first_and_replays_its_response() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let gate = gate("rush");
    let body = json!({ "label": "rush", "amount": 7 });
    let call = |body: Value| {
        harness
            .dispatcher()
            .call(a1.clone(), ADD, body, keyed("rush-1"))
    };
    let first = call(body.clone());
    let followers = async {
        until("the first call holds the key", async || {
            entered("rush") == 1
        })
        .await;
        let repeat = call(body.clone());
        let changed = call(json!({ "label": "rush", "amount": 8 }));
        let release = async {
            until("both repeats wait for the first", async || {
                waiting(&harness).await == 2
            })
            .await;
            assert_eq!(entered("rush"), 1, "no repeat entered the handler");
            gate.add_permits(1);
        };
        let (repeat, changed, ()) = tokio::join!(repeat, changed, release);
        (repeat, changed)
    };
    let (first, (repeat, changed)) = tokio::join!(first, followers);
    assert!(!first.replay);
    assert!(repeat.replay, "the repeat answers what the first committed");
    assert_eq!(data(&repeat), data(&first));
    assert_eq!(
        changed.error().map(|error| error.code),
        Some(ErrorCode::IdempotencyMismatch)
    );
    assert_eq!(entered("rush"), 1, "the handler ran once");
    assert_eq!(entries(&harness, "rush").await, 1);
    assert_eq!(keys(&harness).await, ["rush-1"]);
    assert_eq!(waiting(&harness).await, 0);
    harness.finish().await;
}

#[tokio::test]
async fn concurrent_repeats_of_a_task_function_spawn_one_job() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let gate = gate("batch");
    let body = json!({ "label": "batch", "amount": 9 });
    let call = || {
        harness
            .dispatcher()
            .call(a1.clone(), EXPORT, body.clone(), keyed("batch-1"))
    };
    let first = call();
    let follower = async {
        until("the first call spawns its job", async || {
            entered("batch") == 1
        })
        .await;
        let repeat = call();
        let release = async {
            until("the repeat waits for the first", async || {
                waiting(&harness).await == 1
            })
            .await;
            gate.add_permits(1);
        };
        tokio::join!(repeat, release).0
    };
    let (first, repeat) = tokio::join!(first, follower);
    assert!(!first.replay);
    assert!(repeat.replay);
    assert_eq!(data(&repeat), data(&first));
    assert_eq!(
        jobs(&harness, "batch").await,
        [woken(&first)],
        "one job for one key, woken once its record committed"
    );
    assert_eq!(keys(&harness).await, ["batch-1"]);

    let missing = harness
        .dispatcher()
        .call(a1.clone(), EXPORT, body.clone(), CallOptions::default())
        .await;
    assert_eq!(
        missing.error().map(|error| error.key.as_str()),
        Some(IDEMPOTENCY_REQUIRED_KEY)
    );
    assert_eq!(jobs(&harness, "batch").await, [woken(&first)]);
    harness.finish().await;
}

#[tokio::test]
async fn a_keyed_spawn_cut_off_before_its_record_commits_leaves_no_job_and_its_retry_queues_one() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let gate = gate("cut");
    let body = json!({ "label": "cut", "amount": 1 });
    let call = || {
        harness
            .dispatcher()
            .call(a1.clone(), QUEUE, body.clone(), keyed("cut-1"))
    };
    let cut = call().await;
    assert_eq!(
        cut.error().map(|error| error.key.as_str()),
        Some("keel.error.timeout")
    );
    assert_eq!(entered("cut"), 1, "the job was queued before the timeout");
    assert!(
        jobs(&harness, "cut").await.is_empty(),
        "the job went with the transaction the timeout dropped"
    );
    assert!(keys(&harness).await.is_empty());

    gate.add_permits(1);
    let retried = call().await;
    assert!(!retried.replay);
    assert_eq!(jobs(&harness, "cut").await, [woken(&retried)]);
    let again = call().await;
    assert!(again.replay);
    assert_eq!(data(&again), data(&retried));
    assert_eq!(
        jobs(&harness, "cut").await,
        [woken(&retried)],
        "the retry queued the one job of the key"
    );
    assert_eq!(keys(&harness).await, ["cut-1"]);
    harness.finish().await;
}

#[tokio::test]
async fn keyed_spawns_beyond_the_pool_size_hold_one_connection_each_and_all_answer() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let calls = (0..CROWD).map(|n| {
        harness.dispatcher().call(
            a1.clone(),
            EXPORT,
            json!({ "label": "crowd", "amount": n }),
            keyed(&format!("crowd-{n}")),
        )
    });
    let replies = tokio::time::timeout(PATIENCE, futures::future::join_all(calls))
        .await
        .expect("no keyed spawn waits for a second connection");
    let mut expected: Vec<String> = replies.iter().map(woken).collect();
    expected.sort();
    assert_eq!(jobs(&harness, "crowd").await, expected);
    assert_eq!(keys(&harness).await.len(), CROWD);
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
            json!({ "label": label, "amount": 5 }),
            keyed(key),
        )
    };
    let vetoed = call("veto", "veto-1").await;
    assert_eq!(vetoed.error().map(|error| error.key.as_str()), Some(VETO));
    assert_eq!(entered("veto"), 1, "the handler queued its job");
    assert!(
        jobs(&harness, "veto").await.is_empty(),
        "the job went with the call's transaction"
    );
    assert!(keys(&harness).await.is_empty());

    let deferred = call("later", "later-1").await;
    assert!(!deferred.replay);
    let expected = [woken(&deferred)];
    until("the job is woken after the commit", async || {
        jobs(&harness, "later").await == expected
    })
    .await;
    let again = call("later", "later-1").await;
    assert!(again.replay);
    assert_eq!(data(&again), data(&deferred));
    assert_eq!(jobs(&harness, "later").await, expected);
    assert_eq!(entered("later"), 1);
    assert_eq!(keys(&harness).await, ["later-1"]);
    harness.finish().await;
}

#[tokio::test]
async fn keel_ctl_sends_the_key_of_its_flag() {
    let harness = boot().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let serving = Socket::new(harness.dispatcher().clone())
        .start(&dir.path().join("core.sock"))
        .await
        .expect("the socket starts");
    let request = |key: Option<&str>| ctl::Request {
        func: OPEN.to_owned(),
        params: json!({ "label": "ctl", "amount": 2 }),
        out: Out::Json,
        lang: "en".to_owned(),
        run_as: None,
        idempotency_key: key.map(str::to_owned),
    };
    let envelope = |output: &ctl::Output| -> Value {
        serde_json::from_str(&output.text).expect("the envelope is JSON")
    };
    let missing = ctl::run(serving.path(), &request(None))
        .await
        .expect("ctl reaches the socket");
    assert!(!missing.ok);
    assert_eq!(
        envelope(&missing)["error"]["key"],
        json!(IDEMPOTENCY_REQUIRED_KEY)
    );
    let first = ctl::run(serving.path(), &request(Some("ctl-1")))
        .await
        .expect("ctl reaches the socket");
    assert!(first.ok, "{}", first.text);
    let again = ctl::run(serving.path(), &request(Some("ctl-1")))
        .await
        .expect("ctl reaches the socket");
    assert!(again.ok, "{}", again.text);
    assert_eq!(envelope(&again)["data"], envelope(&first)["data"]);
    assert_eq!(entries(&harness, "ctl").await, 1, "the repeat replayed");
    assert_eq!(keys(&harness).await, ["ctl-1"]);
    serving.stop().await.expect("the socket stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_anything_runs_and_a_read_ignores_it() {
    let harness = boot().await;
    let http = harness.http().await;
    let body = json!({ "label": "odd", "amount": 4 });
    let long = "k".repeat(IDEMPOTENCY_KEY_MAX_LEN + 1);
    for key in [long.as_str(), "clé"] {
        let answer = post(&http, Who::A1, ADD, Some(key), &body).await;
        let error = refused(
            &answer,
            422,
            ErrorCode::Validation,
            IDEMPOTENCY_MALFORMED_KEY,
        );
        assert_eq!(error["params"], json!({ "max": IDEMPOTENCY_KEY_MAX_LEN }));
    }
    let longest = "k".repeat(IDEMPOTENCY_KEY_MAX_LEN);
    ok(&post(&http, Who::A1, ADD, Some(&longest), &body).await);
    let read = post(&http, Who::A1, SUM, Some(long.as_str()), &json!({})).await;
    assert_eq!(ok(&read), json!({ "total": 0 }));

    let dir = tempfile::tempdir().expect("a temporary directory");
    let serving = Socket::new(harness.dispatcher().clone())
        .start(&dir.path().join("core.sock"))
        .await
        .expect("the socket starts");
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    match client
        .call(ADD, body.clone(), keyed("tab\there"))
        .await
        .expect("the socket answers")
    {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(error.key, IDEMPOTENCY_MALFORMED_KEY);
        }
        Envelope::Ok { data, .. } => panic!("a malformed key ran the call: {data}"),
    }
    serving.stop().await.expect("the socket stops");
    assert_eq!(entered("odd"), 1);
    assert_eq!(entries(&harness, "odd").await, 1);
    assert_eq!(keys(&harness).await, [longest]);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_required_key_on_a_read_or_guest_function_fails_the_freeze() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = keel_kernel::Kernel::boot(Composition::new().module(&MISDECLARED), &storage)
        .await
        .expect_err("a key no call can carry fails the freeze");
    let text = error.to_string();
    assert!(
        text.contains("function misdeclared.entry.sum: it requires an idempotency key, which a read function never records"),
        "{text}"
    );
    assert!(
        text.contains("function misdeclared.entry.claim: it requires an idempotency key, which a guest call cannot carry"),
        "{text}"
    );
    assert!(!text.contains("misdeclared.entry.open"), "{text}");
    storage.close().await.expect("the pool closes");
    database.finish().await;
}
