use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_kernel::dispatcher::{
    BATCH_LOG_PREFIX, GROUP_BODY_KEY, MULTI_FIELD_KEY, MULTI_UNMASKED_KEY,
};
use keel_kernel::{Composition, Kernel};
use keel_sdk::prelude::*;
use keel_sdk::system::{
    AuditEvent, AuditSink, Caller, CallerOptions, TaskRunner, TaskSink, TaskSpec,
};
use keel_sdk::{Registrar, TaskHandle, migrator};
use keel_testkit::{Harness, Reply, TestDatabase, Who};
use keel_types::{ErrorCode, TaskState};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

mod card {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;
    use keel_sdk::serde_json::json;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[serde(rename_all = "camelCase")]
    #[schemars(rename_all = "camelCase")]
    #[sea_orm(table_name = "desk_card")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
        pub color: String,
        pub priority: i32,
        #[schemars(extend("x-keel" = json!({ "secret": true })))]
        pub pin: String,
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
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct CardEdit {
    id: Option<Id>,
    title: String,
    #[serde(default = "white")]
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    color: String,
    #[serde(default)]
    #[schemars(range(min = 0, max = 9))]
    priority: i32,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    account: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    pin: Option<Secret>,
}

fn white() -> String {
    "white".to_owned()
}

#[derive(Default)]
struct Trace {
    handled: Mutex<Vec<(String, Value)>>,
    before: Mutex<Vec<String>>,
    within: Mutex<Vec<String>>,
    audit: Mutex<Vec<AuditEvent>>,
}

impl Trace {
    fn handled(&self, request_id: &str) -> Vec<Value> {
        self.handled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(id, _)| id == request_id)
            .map(|(_, params)| params.clone())
            .collect()
    }

    fn hooks(&self, request_id: &str) -> (usize, usize) {
        let count = |list: &Mutex<Vec<String>>| {
            list.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|id| *id == request_id)
                .count()
        };
        (count(&self.before), count(&self.within))
    }

    fn audit_of(&self, request_id: &str) -> Vec<AuditEvent> {
        self.audit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.request_id == request_id)
            .cloned()
            .collect()
    }
}

impl AuditSink for Trace {
    fn record(&self, event: AuditEvent) {
        self.audit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
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

    async fn quiesce(&self, _: Id) -> Result<Vec<Id>> {
        Ok(Vec::new())
    }

    fn resume(&self) -> Result<()> {
        Ok(())
    }
}

struct SinkDouble {
    id: Id,
    seen: Mutex<Vec<String>>,
}

impl SinkDouble {
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

    async fn progress(&self, done: u64, total: Option<u64>, _: Option<&str>) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("progress {done}/{total:?}"));
        Ok(())
    }

    async fn log(&self, line: &str) -> Result<()> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("log {line}"));
        Ok(())
    }

    async fn checkpoint(&self, _: Value) -> Result<()> {
        Ok(())
    }

    fn resume_from(&self) -> Option<Value> {
        None
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn output_path(&self) -> PathBuf {
        PathBuf::from("/nonexistent")
    }

    fn stored(&self, _: DownloadFile) {}
}

static TRACE: LazyLock<Arc<Trace>> = LazyLock::new(Arc::default);
static RUNNER: LazyLock<Arc<RunnerDouble>> = LazyLock::new(Arc::default);

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/kernel/modules/desk/migrations")
        .table("keel_migration_desk");

static DESK: ModuleDef = ModuleDef {
    id: "desk",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

static MISNAMED: ModuleDef = ModuleDef {
    id: "desk",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init: misnamed_init,
};

const EN: &str = r#"{ "desk": { "card": {
    "edit": { "title": "Card" }, "title": "Title", "color": "Colour", "priority": "Priority",
    "account": "Account", "pin": "Pin", "error": { "taken": "The colour {color} is taken" } } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    r.function(get())
        .function(edit(&["color", "priority", "account"]))
        .permission("desk.card.manage", &[Level::User, Level::Reseller])
        .i18n("en", EN)
        .provide::<dyn AuditSink>(TRACE.clone())
        .provide::<dyn TaskRunner>(RUNNER.clone())
        .before("desk.card.edit", |call: Call<Value>| async move {
            TRACE
                .before
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call.request_id().to_owned());
            Ok(Before::Proceed)
        })
        .within("desk.card.edit", |call: Call<Value>| async move {
            call.tx()?;
            TRACE
                .within
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(call.request_id().to_owned());
            Ok(())
        });
    Ok(())
}

fn misnamed_init(r: &mut Registrar) -> Result<()> {
    r.function(get())
        .function(edit(&["id", "pin", "ghost", "priority"]))
        .permission("desk.card.manage", &[Level::User, Level::Reseller])
        .i18n("en", EN);
    Ok(())
}

fn get() -> FunctionDef {
    function::<Pick, card::Model>("desk.card.get")
        .kind(Kind::Get)
        .access(Level::User, "desk.card.manage")
        .owned::<card::Entity>()
        .handler(|call: Call<Pick>| async move {
            card::Entity::find_by_id(call.params().id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
        })
}

fn edit(multi: &[&str]) -> FunctionDef {
    function::<CardEdit, card::Model>("desk.card.edit")
        .kind(Kind::Form)
        .access(Level::User, "desk.card.manage")
        .owned::<card::Entity>()
        .batch_threshold(2)
        .ui(Form::new("desk.card.edit.title")
            .read("desk.card.get")
            .page("main", ["title", "color", "priority", "account", "pin"])
            .multi(multi.iter().copied()))
        .handler(|call: Call<CardEdit>| async move {
            let params = call.params();
            TRACE
                .handled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    call.request_id().to_owned(),
                    json!({ "id": params.id, "title": params.title, "color": params.color, "priority": params.priority }),
                ));
            if params.color == "boom" {
                return Err(KeelError::conflict("desk.card.error.taken")
                    .field("color")
                    .param("color", "boom"));
            }
            let tx = call.tx()?;
            let current = match params.id {
                Some(id) => card::Entity::find_by_id(id.to_string()).one(tx).await?,
                None => None,
            };
            let account = match (&params.account, &current) {
                (None, Some(row)) => row.account_id.clone(),
                (requested, _) => call.resolve_account(*requested).await?.to_string(),
            };
            let id = params.id.unwrap_or_else(Id::generate).to_string();
            let pin = match (&params.pin, &current) {
                (Some(pin), _) => pin.expose().to_owned(),
                (None, Some(row)) => row.pin.clone(),
                (None, None) => String::new(),
            };
            let row = card::ActiveModel {
                id: Set(id.clone()),
                account_id: Set(account),
                title: Set(params.title.clone()),
                color: Set(params.color.clone()),
                priority: Set(params.priority),
                pin: Set(pin),
            };
            let saved = match current {
                Some(_) => row.update(tx).await?,
                None => row.insert(tx).await?,
            };
            call.emit("desk.card.saved", &json!({ "id": id }))?;
            Ok(saved)
        })
}

async fn boot() -> Harness {
    Harness::new().module(&DESK).boot().await
}

async fn create(harness: &Harness, who: Who, title: &str) -> Id {
    let reply = harness
        .call(
            who,
            "desk.card.edit",
            json!({ "title": title, "priority": 1, "pin": "1234" }),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("desk.card.edit failed: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

async fn cards(harness: &Harness, who: Who) -> Vec<(String, String, i32, String)> {
    card::Entity::find()
        .filter(card::Column::AccountId.eq(harness.principal(who).account_id.to_string()))
        .order_by_asc(card::Column::Title)
        .all(harness.db())
        .await
        .expect("cards")
        .into_iter()
        .map(|row| (row.title, row.color, row.priority, row.pin))
        .collect()
}

fn group(ids: &[Id], fields: Value, mask: &[&str]) -> Value {
    json!({ "ids": ids, "fields": fields, "mask": mask })
}

fn fields_of(reply: &Reply) -> Vec<(String, String, Value)> {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()));
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(error.status(), 422);
    error
        .fields
        .iter()
        .map(|field| {
            (
                field.path.clone(),
                field.key.clone().unwrap_or_default(),
                field.params.get("field").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

#[keel_testkit::tokio::test]
async fn a_group_edit_merges_the_masked_fields_over_ui_read_into_every_row_inside_one_transaction()
{
    let harness = boot().await;
    let alpha = create(&harness, Who::A1, "alpha").await;
    let beta = create(&harness, Who::A1, "beta").await;
    let request_id = "req-group-1";
    let reply = harness
        .call_with(
            harness.principal(Who::A1),
            "desk.card.edit",
            group(
                &[alpha, beta],
                json!({ "color": "red", "priority": 5 }),
                &["color", "priority"],
            ),
            keel_testkit::CallOptions {
                request_id: Some(request_id.to_owned()),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(
        reply.data(),
        Some(&json!({ "updated": 2 })),
        "{:?}",
        reply.error()
    );
    assert_eq!(
        cards(&harness, Who::A1).await,
        [
            ("alpha".to_owned(), "red".to_owned(), 5, "1234".to_owned()),
            ("beta".to_owned(), "red".to_owned(), 5, "1234".to_owned()),
        ]
    );
    let handled = TRACE.handled(request_id);
    assert_eq!(handled.len(), 2);
    for (params, id) in handled.iter().zip([alpha, beta]) {
        assert_eq!(params["id"], json!(id.to_string()));
        assert_eq!(params["color"], json!("red"));
        assert_eq!(params["priority"], json!(5));
    }
    assert_eq!(handled[0]["title"], json!("alpha"));
    assert_eq!(handled[1]["title"], json!("beta"));
    assert_eq!(TRACE.hooks(request_id), (2, 2));
    let events = harness
        .database()
        .rows("SELECT coalesce(subject_json->>'id', '-') FROM keel_outbox WHERE type = 'desk.card.saved' AND data_json->>'id' IN (SELECT id FROM desk_card WHERE color = 'red') ORDER BY seq")
        .await;
    assert_eq!(
        events,
        [
            "-".to_owned(),
            "-".to_owned(),
            alpha.to_string(),
            beta.to_string()
        ],
        "the creates carry no subject, the group edits the derived one"
    );
    let audited = TRACE.audit_of(request_id);
    assert_eq!(audited.len(), 1);
    assert!(audited[0].outcome.is_committed());
    assert_eq!(audited[0].params["mask"], json!(["color", "priority"]));
    assert_eq!(
        audited[0].subject.as_deref(),
        Some(format!("[\"{alpha}\",\"{beta}\"]").as_str())
    );
    assert_eq!(audited[0].result, Some(json!({ "updated": 2 })));

    let failing = "req-group-boom";
    let reply = harness
        .call_with(
            harness.principal(Who::A1),
            "desk.card.edit",
            group(&[alpha, beta], json!({ "color": "boom" }), &["color"]),
            keel_testkit::CallOptions {
                request_id: Some(failing.to_owned()),
                ..Default::default()
            },
        )
        .await;
    let fields = fields_of(&reply);
    assert_eq!(fields[0].0, "color");
    assert_eq!(fields[0].1, "desk.card.error.taken");
    assert_eq!(
        cards(&harness, Who::A1).await,
        [
            ("alpha".to_owned(), "red".to_owned(), 5, "1234".to_owned()),
            ("beta".to_owned(), "red".to_owned(), 5, "1234".to_owned()),
        ],
        "the failing row rolled the whole group back"
    );
    assert!(!TRACE.audit_of(failing)[0].outcome.is_committed());

    let reseller = harness
        .call(
            Who::Reseller,
            "desk.card.edit",
            group(
                &[alpha],
                json!({ "account": harness.fixtures().accounts.a2 }),
                &["account"],
            ),
        )
        .await;
    assert_eq!(
        reseller.data(),
        Some(&json!({ "updated": 1 })),
        "{:?}",
        reseller.error()
    );
    assert_eq!(cards(&harness, Who::A1).await.len(), 1);
    assert_eq!(cards(&harness, Who::A2).await.len(), 1);
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_mask_outside_ui_multi_or_a_field_outside_the_mask_answers_422_on_the_offending_path() {
    let harness = boot().await;
    let alpha = create(&harness, Who::A1, "alpha").await;
    let reply = harness
        .call(
            Who::A1,
            "desk.card.edit",
            group(
                &[alpha],
                json!({ "color": "red", "priority": 3 }),
                &["color", "title", "account"],
            ),
        )
        .await;
    assert_eq!(
        fields_of(&reply),
        [
            (
                "mask[1]".to_owned(),
                MULTI_FIELD_KEY.to_owned(),
                json!("title")
            ),
            (
                "mask[2]".to_owned(),
                MULTI_FIELD_KEY.to_owned(),
                json!("account")
            ),
            (
                "fields.priority".to_owned(),
                MULTI_UNMASKED_KEY.to_owned(),
                json!("priority")
            ),
        ]
    );
    let invalid = harness
        .call(
            Who::A1,
            "desk.card.edit",
            group(
                &[alpha],
                json!({ "color": "not a name!", "priority": 12 }),
                &["color", "priority"],
            ),
        )
        .await;
    let fields = fields_of(&invalid);
    assert_eq!(fields[0].0, "fields.priority");
    assert_eq!(fields[0].1, "keel.schema.maximum");
    assert_eq!(fields[1].0, "fields.color");
    assert_eq!(fields[1].1, "keel.check.name");
    let malformed = harness
        .call(Who::A1, "desk.card.edit", json!({ "ids": "one" }))
        .await;
    assert_eq!(
        malformed.error().map(|error| error.key.as_str()),
        Some(GROUP_BODY_KEY)
    );
    assert_eq!(
        cards(&harness, Who::A1).await,
        [("alpha".to_owned(), "white".to_owned(), 1, "1234".to_owned())]
    );
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_group_edit_with_one_foreign_id_is_not_found_and_changes_nothing() {
    let harness = boot().await;
    let mine = create(&harness, Who::A1, "mine").await;
    let theirs = create(&harness, Who::Unrelated, "theirs").await;
    let reply = harness
        .call(
            Who::A1,
            "desk.card.edit",
            group(&[mine, theirs], json!({ "color": "red" }), &["color"]),
        )
        .await;
    assert_eq!(
        reply.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    assert_eq!(
        cards(&harness, Who::A1).await,
        [("mine".to_owned(), "white".to_owned(), 1, "1234".to_owned())]
    );
    assert_eq!(
        cards(&harness, Who::Unrelated).await,
        [(
            "theirs".to_owned(),
            "white".to_owned(),
            1,
            "1234".to_owned()
        )]
    );
    let mut ids = vec![mine];
    for index in 0..3 {
        ids.push(create(&harness, Who::A1, &format!("card {index}")).await);
    }
    ids.push(theirs);
    let above = harness
        .call(
            Who::A1,
            "desk.card.edit",
            group(&ids, json!({ "color": "red" }), &["color"]),
        )
        .await;
    assert_eq!(
        above.error().map(|error| error.code),
        Some(ErrorCode::NotFound)
    );
    assert!(
        RUNNER.spawned_for(harness.principal(Who::A1)).is_empty(),
        "a foreign id fails the batch before any job"
    );
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_group_edit_above_the_threshold_becomes_a_job_of_chunks() {
    let harness = boot().await;
    let mut ids = Vec::new();
    for index in 0..5 {
        ids.push(create(&harness, Who::A2, &format!("card {index}")).await);
    }
    let caller = harness.principal(Who::A2).clone();
    let reply = harness
        .call(
            Who::A2,
            "desk.card.edit",
            group(&ids, json!({ "priority": 7 }), &["priority"]),
        )
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("a task ref was expected: {:?}", reply.error()));
    let id: Id = data["task"]["id"]
        .as_str()
        .expect("the task id")
        .parse()
        .expect("a ulid");
    assert!(
        cards(&harness, Who::A2)
            .await
            .iter()
            .all(|(_, _, priority, _)| *priority == 1),
        "the group did not run inline"
    );
    let spawned = RUNNER.spawned_for(&caller);
    assert_eq!(spawned.len(), 1);
    let (spawned_id, spec) = &spawned[0];
    assert_eq!(*spawned_id, id);
    assert_eq!(spec.function, "desk.card.edit");
    assert_eq!(
        spec.params,
        group(&ids, json!({ "priority": 7 }), &["priority"])
    );

    let sink = Arc::new(SinkDouble {
        id,
        seen: Mutex::new(Vec::new()),
    });
    let service = harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    let outcome = service
        .call(
            spec.principal.clone(),
            &spec.function,
            spec.params.clone(),
            CallerOptions {
                request_id: Some(format!("task-{id}")),
                task: Some(TaskHandle::new(sink.clone())),
            },
        )
        .await
        .expect("the chunks run");
    assert_eq!(
        outcome,
        json!([{ "updated": 2 }, { "updated": 2 }, { "updated": 1 }])
    );
    assert!(
        cards(&harness, Who::A2)
            .await
            .iter()
            .all(|(_, _, priority, _)| *priority == 7)
    );
    assert_eq!(
        sink.seen(),
        [
            "progress 2/Some(5)",
            &format!("log {BATCH_LOG_PREFIX} 1/3: 2 ids"),
            "progress 4/Some(5)",
            &format!("log {BATCH_LOG_PREFIX} 2/3: 2 ids"),
            "progress 5/Some(5)",
            &format!("log {BATCH_LOG_PREFIX} 3/3: 1 ids"),
        ]
    );
    assert_eq!(RUNNER.spawned_for(&caller).len(), 1);
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn ui_multi_naming_the_key_a_secret_or_an_unknown_field_fails_freeze() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&MISNAMED), &storage)
        .await
        .expect_err("ui.multi names the key, a secret and an unknown field");
    let text = error.to_string();
    assert!(
        text.contains("function desk.card.edit: ui.multi names id, which is the key"),
        "{text}"
    );
    assert!(
        text.contains("function desk.card.edit: ui.multi names pin, which is secret"),
        "{text}"
    );
    assert!(
        text.contains("function desk.card.edit: ui.multi names ghost, which is not a parameter"),
        "{text}"
    );
    assert!(!text.contains("ui.multi names priority"), "{text}");
    storage.close().await.expect("the pool closes");
    database.finish().await;
}
