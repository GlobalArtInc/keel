use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use keel_kernel::dispatcher::{self, CallOptions, Dispatcher, Reply};
use keel_kernel::{Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink, Client};
use keel_sdk::{Registrar, migrator};
use keel_testkit::TestDatabase;
use keel_types::{Check, ErrorCode, Scope, Via};
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set};

pub const ROOT: &str = "00000000000000000000000000";
pub const RESELLER: &str = "01J8A0000000000000000000A1";
pub const CUSTOMER: &str = "01J8A0000000000000000000A2";
pub const OTHER: &str = "01J8A0000000000000000000A3";

mod book {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;
    use keel_sdk::serde_json::json;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "shelf_book")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
        pub pages: i32,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        pub note: String,
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
struct BookEdit {
    id: Option<Id>,
    title: String,
    pages: i32,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    account: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    note: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
    site: Option<String>,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    token: Option<Secret>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Saved {
    id: Id,
    #[schemars(extend("x-keel" = json!({ "secret": true })))]
    token: Option<String>,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    receipt: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Relay {
    title: String,
    token: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Batch {
    ids: Vec<Id>,
    pages: i32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pair {
    first: String,
    second: String,
    fail: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Probe {
    #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
    domain: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "ip" })))]
    ip: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "cidr" })))]
    cidr: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "email" })))]
    email: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "int" })))]
    int: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    name: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "port" })))]
    port: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "path" })))]
    path: Option<String>,
    hosts: Option<Vec<Host>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Host {
    #[schemars(extend("x-keel" = json!({ "check": "ip" })))]
    ip: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Peek {
    transaction: bool,
    tx_error: Option<String>,
}

#[derive(Default)]
struct Recorder {
    audit: Mutex<Vec<AuditEvent>>,
    after: Mutex<Vec<(String, String)>>,
    before_saw_transaction: Mutex<Vec<(String, bool)>>,
}

impl AuditSink for Recorder {
    fn record(&self, event: AuditEvent) {
        self.audit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Recorder {
    fn audit_of(&self, request_id: &str) -> Vec<AuditEvent> {
        self.audit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.request_id == request_id)
            .cloned()
            .collect()
    }

    fn after_of(&self, request_id: &str) -> Vec<String> {
        self.after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(id, _)| id == request_id)
            .map(|(_, title)| title.clone())
            .collect()
    }
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);

struct Limited;

#[async_trait]
impl Guard for Limited {
    async fn check(&self, _: &str, call: &Call<Value>) -> Result<()> {
        if call.principal().login == "limited" {
            return Err(KeelError::rate_limited("shelf.error.limited"));
        }
        Ok(())
    }
}

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/modules/shelf/migrations")
        .table("keel_migration_shelf");

static SHELF: ModuleDef = ModuleDef {
    id: "shelf",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

const EN: &str = r#"{ "shelf": { "book": {
    "list": { "title": "Books" }, "edit": { "title": "Book" }, "empty": "No books",
    "pages": "Pages", "account": "Account", "note": "Note", "site": "Site", "token": "Token",
    "column": { "pages": "Pages", "account_id": "Account" } } } }"#;

fn init(r: &mut Registrar) -> Result<()> {
    r.function(list())
        .function(get())
        .function(edit())
        .function(batch())
        .function(pair())
        .function(relay())
        .function(probe())
        .function(peek())
        .permission("shelf.book.read", &[Level::User, Level::Reseller])
        .permission("shelf.book.manage", &[Level::User, Level::Reseller])
        .permission("shelf.book.pair", &[Level::Reseller])
        .i18n("en", EN)
        .guard(Arc::new(Limited))
        .provide::<dyn AuditSink>(RECORDER.clone())
        .before("shelf.book.edit", |call: Call<Value>| async move {
            let title = call.params()["title"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            RECORDER
                .before_saw_transaction
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((call.request_id().to_owned(), call.tx().is_ok()));
            match title.as_str() {
                "veto" => Err(KeelError::conflict("shelf.book.error.veto")
                    .field("title")
                    .param("title", title)),
                "enrich" => Ok(Before::Enrich(json!({ "pages": 42 }))),
                "enrich-list" => Ok(Before::Enrich(json!(["pages", 42]))),
                "slow" => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(Before::Proceed)
                }
                _ => Ok(Before::Proceed),
            }
        })
        .within("shelf.book.edit", |call: Call<Value>| async move {
            call.tx()?;
            if call.params()["title"] == json!("within") {
                return Err(KeelError::conflict("shelf.book.error.within"));
            }
            Ok(())
        });
    Ok(())
}

fn list() -> FunctionDef {
    function::<ListQuery, Page<book::Model>>("shelf.book.list")
        .kind(Kind::List)
        .access(Level::User, "shelf.book.read")
        .owned::<book::Entity>()
        .ui(List::new("shelf.book.list.title")
            .search(["title"])
            .column(Column::new("title", ColumnType::Text).sortable().primary())
            .column(
                Column::new("pages", ColumnType::Number)
                    .sortable()
                    .total(Total::Sum),
            )
            .column(Column::new("account_id", ColumnType::Text).level(Level::Reseller))
            .empty("shelf.book.empty", None))
        .default_sort("title", SortDir::Asc)
        .max_page_size(2)
        .source(entity_source::<book::Entity>())
}

fn get() -> FunctionDef {
    function::<Pick, book::Model>("shelf.book.get")
        .kind(Kind::Get)
        .access(Level::User, "shelf.book.read")
        .owned::<book::Entity>()
        .handler(|call: Call<Pick>| async move {
            book::Entity::find_by_id(call.params().id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
        })
}

fn edit() -> FunctionDef {
    function::<BookEdit, Saved>("shelf.book.edit")
        .kind(Kind::Form)
        .access(Level::User, "shelf.book.manage")
        .owned::<book::Entity>()
        .ui(Form::new("shelf.book.edit.title")
            .read("shelf.book.get")
            .page(
                "main",
                ["title", "pages", "account", "note", "site", "token"],
            ))
        .handler(|call: Call<BookEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            let id = call.params().id.unwrap_or_else(Id::generate);
            let row = book::ActiveModel {
                id: Set(id.to_string()),
                account_id: Set(account.to_string()),
                title: Set(call.params().title.clone()),
                pages: Set(call.params().pages),
                note: Set(call.params().note.clone().unwrap_or_default()),
            };
            let saved = match call.params().id {
                Some(_) => row.update(call.tx()?).await?,
                None => row.insert(call.tx()?).await.map_err(|error| {
                    KeelError::conflict("shelf.book.error.exists")
                        .field("title")
                        .source(error)
                })?,
            };
            call.emit("shelf.book.saved", &saved)?;
            let title = saved.title.clone();
            call.after_commit(move |ctx| async move {
                RECORDER
                    .after
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((ctx.request_id().to_owned(), title));
                Ok(())
            });
            if saved.title == "boom" {
                return Err(KeelError::conflict("shelf.book.error.boom"));
            }
            Ok(Saved {
                id,
                token: call
                    .params()
                    .token
                    .as_ref()
                    .map(|token| token.expose().to_owned()),
                receipt: format!("rcpt-{}", saved.title),
            })
        })
}

fn batch() -> FunctionDef {
    function::<Batch, Nothing>("shelf.book.batch")
        .kind(Kind::Action)
        .access(Level::User, "shelf.book.manage")
        .key("ids")
        .owned::<book::Entity>()
        .handler(|call: Call<Batch>| async move {
            for id in &call.params().ids {
                book::Entity::update_many()
                    .col_expr(book::Column::Pages, call.params().pages.into())
                    .filter(book::Column::Id.eq(id.to_string()))
                    .exec(call.tx()?)
                    .await?;
            }
            Ok(Nothing {})
        })
}

fn pair() -> FunctionDef {
    function::<Pair, Nothing>("shelf.book.pair")
        .kind(Kind::Action)
        .access(Level::User, "shelf.book.pair")
        .unowned()
        .handler(|call: Call<Pair>| async move {
            for title in [&call.params().first, &call.params().second] {
                call.invoke("shelf.book.edit", json!({ "title": title, "pages": 1 }))
                    .await?;
            }
            if call.params().fail {
                return Err(KeelError::conflict("shelf.book.error.pair"));
            }
            Ok(Nothing {})
        })
}

fn relay() -> FunctionDef {
    function::<Relay, Value>("shelf.book.relay")
        .kind(Kind::Action)
        .access(Level::User, "shelf.book.manage")
        .unowned()
        .handler(|call: Call<Relay>| async move {
            let params = call.params();
            call.invoke(
                "shelf.book.edit",
                json!({ "title": params.title, "pages": 1, "token": params.token }),
            )
            .await
        })
}

fn probe() -> FunctionDef {
    function::<Probe, Nothing>("shelf.book.probe")
        .kind(Kind::Get)
        .level(Level::User)
        .unowned()
        .handler(|_: Call<Probe>| async { Ok(Nothing {}) })
}

fn peek() -> FunctionDef {
    function::<Nothing, Peek>("shelf.book.peek")
        .kind(Kind::Get)
        .level(Level::User)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            Ok(Peek {
                transaction: call.db().is_transaction(),
                tx_error: call.tx().err().map(|error| error.key().to_owned()),
            })
        })
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Dispatcher,
}

async fn bench() -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(Composition::new().module(&SHELF), &storage)
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
    let dispatcher = Dispatcher::new(kernel.registry().clone(), storage.db().clone())
        .expect("every schema compiles")
        .hook_timeout(Duration::from_millis(200));
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
    }
}

impl Bench {
    async fn call(&self, level: Level, account: &str, func: &str, params: Value) -> Reply {
        self.dispatcher
            .call(
                principal(level, account),
                func,
                params,
                CallOptions::default(),
            )
            .await
    }

    async fn call_with(
        &self,
        principal: Principal,
        func: &str,
        params: Value,
        options: CallOptions,
    ) -> Reply {
        self.dispatcher.call(principal, func, params, options).await
    }

    async fn save(&self, level: Level, account: &str, params: Value) -> Id {
        let reply = self.call(level, account, "shelf.book.edit", params).await;
        let data = reply
            .data()
            .unwrap_or_else(|| panic!("the book saves: {:?}", reply.error()));
        data["id"].as_str().expect("id").parse().expect("ulid")
    }

    async fn count(&self, sql: &str) -> i64 {
        self.database.count(&self.storage, sql).await
    }

    async fn titles(&self) -> Vec<String> {
        book::Entity::find()
            .all(self.storage.db())
            .await
            .expect("books")
            .into_iter()
            .map(|book| book.title)
            .collect()
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
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

fn before_saw_transaction(request_id: &str) -> Vec<bool> {
    RECORDER
        .before_saw_transaction
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(id, _)| id == request_id)
        .map(|(_, transaction)| *transaction)
        .collect()
}

fn request(id: &str) -> CallOptions {
    CallOptions {
        request_id: Some(id.to_owned()),
        ..CallOptions::default()
    }
}

fn code(reply: &Reply) -> ErrorCode {
    reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.data()))
        .code
}

fn paths(reply: &Reply) -> Vec<String> {
    reply
        .error()
        .expect("an error")
        .fields
        .iter()
        .map(|field| field.path.clone())
        .collect()
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the condition did not become true in time");
}

#[tokio::test]
async fn guards_run_in_order_and_each_failure_is_observable_by_its_error_code() {
    let bench = bench().await;
    let params = json!({ "first": "a", "second": "b", "fail": false });

    let unknown = bench
        .call(Level::Admin, ROOT, "shelf.book.none", json!({}))
        .await;
    assert_eq!(code(&unknown), ErrorCode::NotFound);
    assert_eq!(unknown.status(), 404);

    let guest = bench
        .call(Level::Guest, ROOT, "shelf.book.pair", params.clone())
        .await;
    assert_eq!(code(&guest), ErrorCode::Unauthenticated);
    assert_eq!(guest.status(), 401);

    let service = bench
        .call(Level::Service, CUSTOMER, "shelf.book.pair", params.clone())
        .await;
    assert_eq!(code(&service), ErrorCode::Forbidden);
    assert_eq!(service.error().unwrap().key, "keel.error.level");

    let user = bench
        .call(Level::User, CUSTOMER, "shelf.book.pair", params.clone())
        .await;
    assert_eq!(code(&user), ErrorCode::Forbidden);
    assert_eq!(user.error().unwrap().key, "keel.error.permission");
    assert_eq!(
        user.error().unwrap().params["permission"],
        "shelf.book.pair"
    );

    let mut limited_user = principal(Level::User, CUSTOMER);
    limited_user.login = "limited".into();
    let permission_first = bench
        .call_with(
            limited_user,
            "shelf.book.pair",
            params.clone(),
            CallOptions::default(),
        )
        .await;
    assert_eq!(code(&permission_first), ErrorCode::Forbidden);

    let mut limited = principal(Level::Reseller, RESELLER);
    limited.login = "limited".into();
    let limited = bench
        .call_with(
            limited,
            "shelf.book.pair",
            params.clone(),
            CallOptions::default(),
        )
        .await;
    assert_eq!(code(&limited), ErrorCode::RateLimited);
    assert_eq!(limited.status(), 429);

    let id = bench
        .save(
            Level::User,
            OTHER,
            json!({ "title": "foreign", "pages": 1 }),
        )
        .await;
    let mut limited = principal(Level::Reseller, RESELLER);
    limited.login = "limited".into();
    let guarded_before_owned = bench
        .call_with(
            limited,
            "shelf.book.get",
            json!({ "id": id }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(code(&guarded_before_owned), ErrorCode::RateLimited);
    let owned = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.get",
            json!({ "id": id }),
        )
        .await;
    assert_eq!(code(&owned), ErrorCode::NotFound);

    let admin = bench
        .call(Level::Admin, ROOT, "shelf.book.pair", params)
        .await;
    assert!(admin.data().is_some(), "{:?}", admin.error());
    let reseller = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.pair",
            json!({ "first": "c", "second": "d", "fail": false }),
        )
        .await;
    assert!(reseller.data().is_some(), "{:?}", reseller.error());
    bench.finish().await;
}

#[tokio::test]
async fn the_scope_guard_runs_after_level_and_permission_before_module_guards_and_not_for_nested_invokes()
 {
    let bench = bench().await;
    let scoped = |level: Level, login: &str, masks: &[&str]| {
        let mut principal = principal(level, CUSTOMER);
        principal.login = login.into();
        principal.scope = Scope::Functions(masks.iter().map(|mask| (*mask).to_owned()).collect());
        principal
    };
    let listing = scoped(Level::User, "scoped", &["shelf.book.list"]);
    let admitted = bench
        .call_with(
            listing.clone(),
            "shelf.book.list",
            json!({}),
            CallOptions::default(),
        )
        .await;
    assert!(admitted.data().is_some(), "{:?}", admitted.error());
    let outside = bench
        .call_with(
            listing.clone(),
            "shelf.book.edit",
            json!({ "title": "no", "pages": 1 }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(code(&outside), ErrorCode::Forbidden);
    assert_eq!(outside.error().unwrap().key, "keel.error.token_scope");
    assert_eq!(
        outside.error().unwrap().params["function"],
        "shelf.book.edit"
    );
    assert_eq!(bench.titles().await, Vec::<String>::new());

    let level_first = bench
        .call_with(
            scoped(Level::Service, "scoped", &["shelf.book.list"]),
            "shelf.book.pair",
            json!({ "first": "a", "second": "b", "fail": false }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(level_first.error().unwrap().key, "keel.error.level");
    let permission_first = bench
        .call_with(
            listing,
            "shelf.book.pair",
            json!({ "first": "a", "second": "b", "fail": false }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(
        permission_first.error().unwrap().key,
        "keel.error.permission"
    );
    let scope_before_module_guards = bench
        .call_with(
            scoped(Level::User, "limited", &["shelf.book.list"]),
            "shelf.book.edit",
            json!({ "title": "no", "pages": 1 }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(
        scope_before_module_guards.error().unwrap().key,
        "keel.error.token_scope"
    );

    let mut reader = principal(Level::User, CUSTOMER);
    reader.scope = Scope::ReadOnly;
    let reads = bench
        .call_with(
            reader.clone(),
            "shelf.book.list",
            json!({}),
            CallOptions::default(),
        )
        .await;
    assert!(reads.data().is_some(), "{:?}", reads.error());
    let writes = bench
        .call_with(
            reader,
            "shelf.book.edit",
            json!({ "title": "no", "pages": 1 }),
            CallOptions::default(),
        )
        .await;
    assert_eq!(code(&writes), ErrorCode::Forbidden);
    assert_eq!(writes.error().unwrap().key, "keel.error.token_read_only");
    assert_eq!(bench.titles().await, Vec::<String>::new());

    let relayed = bench
        .call_with(
            scoped(Level::User, "scoped", &["shelf.book.relay"]),
            "shelf.book.relay",
            json!({ "title": "nested", "token": "t" }),
            CallOptions::default(),
        )
        .await;
    assert!(relayed.data().is_some(), "{:?}", relayed.error());
    assert_eq!(bench.titles().await, ["nested"]);
    bench.finish().await;
}

#[tokio::test]
async fn owned_guard_answers_not_found_for_a_foreign_row_and_for_a_batch_with_one_foreign_id() {
    let bench = bench().await;
    let mine = bench
        .save(
            Level::User,
            CUSTOMER,
            json!({ "title": "mine", "pages": 1 }),
        )
        .await;
    let theirs = bench
        .save(Level::User, OTHER, json!({ "title": "theirs", "pages": 1 }))
        .await;

    let edit = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "id": theirs, "title": "stolen", "pages": 9 }),
        )
        .await;
    assert_eq!(code(&edit), ErrorCode::NotFound);
    assert_eq!(edit.status(), 404);
    let get = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.get",
            json!({ "id": theirs }),
        )
        .await;
    assert_eq!(code(&get), ErrorCode::NotFound);
    let missing = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.get",
            json!({ "id": Id::generate() }),
        )
        .await;
    assert_eq!(code(&missing), ErrorCode::NotFound);

    let batch = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.batch",
            json!({ "ids": [mine, theirs], "pages": 7 }),
        )
        .await;
    assert_eq!(code(&batch), ErrorCode::NotFound);
    let own_batch = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.batch",
            json!({ "ids": [mine, mine], "pages": 7 }),
        )
        .await;
    assert!(own_batch.data().is_some(), "{:?}", own_batch.error());

    let reseller = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.get",
            json!({ "id": mine }),
        )
        .await;
    assert_eq!(reseller.data().unwrap()["pages"], json!(7));
    let outside = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.get",
            json!({ "id": theirs }),
        )
        .await;
    assert_eq!(code(&outside), ErrorCode::NotFound);
    let admin = bench
        .call(
            Level::Admin,
            ROOT,
            "shelf.book.get",
            json!({ "id": theirs }),
        )
        .await;
    assert_eq!(admin.data().unwrap()["title"], json!("theirs"));
    assert_eq!(bench.titles().await.len(), 2);
    bench.finish().await;
}

#[tokio::test]
async fn strip_removes_fields_above_the_level_from_results_and_refuses_them_on_writes() {
    let bench = bench().await;
    let id = bench
        .save(
            Level::Admin,
            ROOT,
            json!({ "title": "annotated", "pages": 3, "account": CUSTOMER, "note": "private" }),
        )
        .await;

    let user = bench
        .call(Level::User, CUSTOMER, "shelf.book.get", json!({ "id": id }))
        .await;
    let row = user.data().unwrap();
    assert_eq!(row["title"], json!("annotated"));
    assert!(row.get("note").is_none(), "{row}");
    let admin = bench
        .call(Level::Admin, ROOT, "shelf.book.get", json!({ "id": id }))
        .await;
    assert_eq!(admin.data().unwrap()["note"], json!("private"));

    let listed = bench
        .call(Level::User, CUSTOMER, "shelf.book.list", json!({}))
        .await;
    let first = &listed.data().unwrap()["data"][0];
    assert!(first.get("note").is_none(), "{first}");
    assert!(first.get("account_id").is_none(), "{first}");
    let listed = bench
        .call(Level::Reseller, RESELLER, "shelf.book.list", json!({}))
        .await;
    let first = &listed.data().unwrap()["data"][0];
    assert_eq!(first["account_id"], json!(CUSTOMER));
    assert!(first.get("note").is_none(), "{first}");

    let refused = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "x", "pages": 1, "account": OTHER, "note": "n" }),
        )
        .await;
    assert_eq!(code(&refused), ErrorCode::ForbiddenField);
    assert_eq!(refused.status(), 403);
    assert_eq!(paths(&refused), ["account", "note"]);
    let reseller = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.edit",
            json!({ "title": "x", "pages": 1, "account": CUSTOMER, "note": "n" }),
        )
        .await;
    assert_eq!(paths(&reseller), ["note"]);
    assert_eq!(bench.titles().await, ["annotated"]);
    bench.finish().await;
}

#[tokio::test]
async fn validation_answers_422_with_field_paths_and_check_codes_from_the_fixture_cases() {
    #[derive(Deserialize)]
    struct Case {
        check: Check,
        value: String,
        expect: bool,
    }
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../contracts/fixtures/check-cases.json"))
            .expect("the fixture parses");
    assert!(cases.len() > 100);
    let bench = bench().await;
    for case in &cases {
        let field = case.check.as_str();
        let reply = bench
            .call(
                Level::User,
                CUSTOMER,
                "shelf.book.probe",
                json!({ field: case.value }),
            )
            .await;
        if case.expect {
            assert!(
                reply.data().is_some(),
                "{field} {:?}: {:?}",
                case.value,
                reply.error()
            );
        } else {
            assert_eq!(
                code(&reply),
                ErrorCode::Validation,
                "{field} {:?}",
                case.value
            );
            assert_eq!(reply.status(), 422);
            let error = reply.error().unwrap();
            assert_eq!(error.fields.len(), 1, "{field} {:?}", case.value);
            assert_eq!(error.fields[0].path, field);
            assert_eq!(
                error.fields[0].code.as_deref(),
                Some(case.check.code().as_str())
            );
            assert_eq!(
                error.fields[0].key.as_deref(),
                Some(case.check.key().as_str())
            );
            assert_eq!(error.fields[0].params["value"], json!(case.value));
        }
    }
    let schema = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "pages": "many", "site": "not a domain" }),
        )
        .await;
    assert_eq!(code(&schema), ErrorCode::Validation);
    let error = schema.error().unwrap();
    assert_eq!(
        error.request_id.as_deref().map(|id| id.starts_with("req-")),
        Some(true)
    );
    let summary: Vec<(&str, &str)> = error
        .fields
        .iter()
        .map(|field| {
            (
                field.path.as_str(),
                field.code.as_deref().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("title", "schema.required"),
            ("pages", "schema.type"),
            ("site", "check.domain"),
        ]
    );
    let nested = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.probe",
            json!({ "hosts": [ { "ip": "10.0.0.1" }, { "ip": "10.0.0" } ] }),
        )
        .await;
    assert_eq!(code(&nested), ErrorCode::Validation);
    assert_eq!(paths(&nested), ["hosts[1].ip"]);
    assert_eq!(
        nested.error().unwrap().fields[0].code.as_deref(),
        Some("check.ip")
    );
    bench.finish().await;
}

#[tokio::test]
async fn idempotency_replays_the_same_body_rejects_a_different_body_and_ignores_reads() {
    let bench = bench().await;
    let alice = principal(Level::User, CUSTOMER);
    let key = || CallOptions {
        idempotency_key: Some("order-1".into()),
        ..CallOptions::default()
    };
    let body = json!({ "title": "once", "pages": 1 });
    let first = bench
        .call_with(alice.clone(), "shelf.book.edit", body.clone(), key())
        .await;
    assert!(!first.replay);
    let id = first.data().unwrap()["id"].clone();

    let again = bench
        .call_with(
            alice.clone(),
            "shelf.book.edit",
            json!({ "pages": 1, "title": "once" }),
            key(),
        )
        .await;
    assert!(again.replay);
    assert_eq!(again.data().unwrap()["id"], id);
    assert_eq!(bench.titles().await, ["once"]);
    assert_eq!(
        bench.count("SELECT count(*) FROM keel_outbox").await,
        1,
        "a replay writes no second event"
    );
    assert_eq!(
        bench.count("SELECT count(*) FROM keel_idempotency").await,
        1
    );

    let changed = bench
        .call_with(
            alice.clone(),
            "shelf.book.edit",
            json!({ "title": "twice", "pages": 1 }),
            key(),
        )
        .await;
    assert_eq!(code(&changed), ErrorCode::IdempotencyMismatch);
    assert_eq!(changed.status(), 409);
    assert_eq!(bench.titles().await, ["once"]);

    let other_function = bench
        .call_with(
            alice.clone(),
            "shelf.book.batch",
            json!({ "ids": [], "pages": 2 }),
            key(),
        )
        .await;
    assert_eq!(code(&other_function), ErrorCode::IdempotencyMismatch);

    let bob = principal(Level::User, CUSTOMER);
    let bobs = bench
        .call_with(
            bob,
            "shelf.book.edit",
            json!({ "title": "bobs", "pages": 1 }),
            key(),
        )
        .await;
    assert!(!bobs.replay, "keys are per principal");
    assert!(bobs.data().is_some());

    let read = bench
        .call_with(alice.clone(), "shelf.book.list", json!({}), key())
        .await;
    assert!(!read.replay);
    let read_again = bench
        .call_with(alice, "shelf.book.list", json!({ "search": "x" }), key())
        .await;
    assert!(!read_again.replay);
    assert!(read_again.data().is_some());
    assert_eq!(
        bench.count("SELECT count(*) FROM keel_idempotency").await,
        2
    );
    bench.finish().await;
}

#[tokio::test]
async fn a_before_hook_runs_without_a_transaction_can_enrich_or_veto_and_is_ended_by_its_timeout() {
    let bench = bench().await;
    let enriched = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "enrich", "pages": 1 }),
            request("before-enrich"),
        )
        .await;
    let id = enriched.data().unwrap()["id"].clone();
    let row = bench
        .call(Level::User, CUSTOMER, "shelf.book.get", json!({ "id": id }))
        .await;
    assert_eq!(row.data().unwrap()["pages"], json!(42));
    assert_eq!(before_saw_transaction("before-enrich"), [false]);

    let vetoed = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "veto", "pages": 1 }),
        )
        .await;
    assert_eq!(code(&vetoed), ErrorCode::Validation);
    let error = vetoed.error().unwrap();
    assert_eq!(error.fields[0].path, "title");
    assert_eq!(
        error.fields[0].key.as_deref(),
        Some("shelf.book.error.veto")
    );
    assert_eq!(error.fields[0].params["title"], json!("veto"));

    let started = std::time::Instant::now();
    let slow = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "slow", "pages": 1 }),
        )
        .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(code(&slow), ErrorCode::Unavailable);
    assert_eq!(slow.error().unwrap().key, "keel.error.hook_timeout");
    assert_eq!(slow.error().unwrap().params["module"], json!("shelf"));
    let not_an_object = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "enrich-list", "pages": 1 }),
        )
        .await;
    assert_eq!(code(&not_an_object), ErrorCode::Internal);
    assert_eq!(not_an_object.status(), 500);
    assert_eq!(
        not_an_object.error().unwrap().key,
        "keel.error.enrich_not_object"
    );
    assert_eq!(
        not_an_object.error().unwrap().params["module"],
        json!("shelf")
    );
    assert_eq!(bench.titles().await, ["enrich"]);
    bench.finish().await;
}

#[tokio::test]
async fn a_within_hook_failure_rolls_back_the_handler_writes() {
    let bench = bench().await;
    let refused = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "within", "pages": 1 }),
        )
        .await;
    assert_eq!(code(&refused), ErrorCode::Conflict);
    assert_eq!(refused.error().unwrap().key, "shelf.book.error.within");
    assert!(bench.titles().await.is_empty());
    assert_eq!(bench.count("SELECT count(*) FROM keel_outbox").await, 0);
    bench.finish().await;
}

#[tokio::test]
async fn a_nested_invoke_shares_the_transaction_so_a_failing_handler_leaves_no_rows() {
    let bench = bench().await;
    let failed = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.pair",
            json!({ "first": "one", "second": "two", "fail": true }),
        )
        .await;
    assert_eq!(code(&failed), ErrorCode::Conflict);
    assert!(bench.titles().await.is_empty());
    assert_eq!(bench.count("SELECT count(*) FROM keel_outbox").await, 0);

    let inner_failed = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.pair",
            json!({ "first": "three", "second": "boom", "fail": false }),
        )
        .await;
    assert_eq!(inner_failed.error().unwrap().key, "shelf.book.error.boom");
    assert!(bench.titles().await.is_empty());

    let inner_refused = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.pair",
            json!({ "first": "four", "second": "veto", "fail": false }),
        )
        .await;
    assert_eq!(code(&inner_refused), ErrorCode::Validation);
    assert!(bench.titles().await.is_empty());

    let done = bench
        .call_with(
            principal(Level::Reseller, RESELLER),
            "shelf.book.pair",
            json!({ "first": "five", "second": "six", "fail": false }),
            request("pair-ok"),
        )
        .await;
    assert!(done.data().is_some(), "{:?}", done.error());
    let mut titles = bench.titles().await;
    titles.sort();
    assert_eq!(titles, ["five", "six"]);
    assert_eq!(bench.count("SELECT count(*) FROM keel_outbox").await, 2);
    wait_for(|| RECORDER.after_of("pair-ok").len() == 2).await;
    assert_eq!(before_saw_transaction("pair-ok"), [false, false]);
    bench.finish().await;
}

#[tokio::test]
async fn an_outbox_row_exists_exactly_when_the_call_committed() {
    let bench = bench().await;
    bench
        .save(
            Level::User,
            CUSTOMER,
            json!({ "title": "kept", "pages": 1 }),
        )
        .await;
    let rolled_back = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "boom", "pages": 1 }),
        )
        .await;
    assert_eq!(rolled_back.error().unwrap().key, "shelf.book.error.boom");
    assert_eq!(
        bench
            .count("SELECT count(*) FROM keel_outbox WHERE type = 'shelf.book.saved'")
            .await,
        1
    );
    let row = bench
        .storage
        .db()
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT seq, id, module, actor_json::text AS actor, data_json::text AS data FROM keel_outbox",
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.try_get::<i64>("", "seq").unwrap(), 1);
    assert_eq!(row.try_get::<String>("", "id").unwrap().len(), 26);
    assert_eq!(row.try_get::<String>("", "module").unwrap(), "shelf");
    let actor: Value = serde_json::from_str(&row.try_get::<String>("", "actor").unwrap()).unwrap();
    assert_eq!(actor["kind"], json!("principal"));
    let data: Value = serde_json::from_str(&row.try_get::<String>("", "data").unwrap()).unwrap();
    assert_eq!(data["title"], json!("kept"));
    bench.finish().await;
}

#[tokio::test]
async fn the_audit_event_carries_the_client_the_subject_and_the_masked_result_of_a_keyed_form() {
    let bench = bench().await;
    let client = Client {
        ip: Some("10.0.0.9".parse().expect("an address")),
        user_agent: Some("probe/1".into()),
        session: None,
    };
    let with_client = |id: &str| CallOptions {
        client: Some(client.clone()),
        ..request(id)
    };
    let created = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "subject", "pages": 1, "token": "hunter2" }),
            with_client("subject-create"),
        )
        .await;
    let id = created.data().expect("the saved book")["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let events = RECORDER.audit_of("subject-create");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].client, Some(client.clone()));
    assert_eq!(events[0].subject, None);
    assert_eq!(
        events[0].result,
        Some(json!({ "id": id, "token": dispatcher::MASK, "receipt": dispatcher::MASK }))
    );

    let edited = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "id": id, "title": "subject-2", "pages": 2 }),
            with_client("subject-edit"),
        )
        .await;
    assert!(edited.data().is_some(), "{:?}", edited.error());
    let events = RECORDER.audit_of("subject-edit");
    assert_eq!(events[0].subject.as_deref(), Some(id.as_str()));
    assert_eq!(events[0].params["id"], json!(id));
    assert_eq!(
        events[0].result,
        Some(json!({ "id": id, "token": null, "receipt": dispatcher::MASK }))
    );

    let failed = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "id": id, "title": "boom", "pages": 3 }),
            with_client("subject-boom"),
        )
        .await;
    assert_eq!(code(&failed), ErrorCode::Conflict);
    let events = RECORDER.audit_of("subject-boom");
    assert_eq!(events[0].subject.as_deref(), Some(id.as_str()));
    assert_eq!(events[0].result, None);

    let batched = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.batch",
            json!({ "ids": [id], "pages": 4 }),
            request("subject-batch"),
        )
        .await;
    assert!(batched.data().is_some(), "{:?}", batched.error());
    let events = RECORDER.audit_of("subject-batch");
    assert_eq!(events[0].client, None);
    assert_eq!(events[0].subject, Some(format!("[\"{id}\"]")));
    assert_eq!(events[0].result, None);
    bench.finish().await;
}

#[tokio::test]
async fn the_audit_event_is_raised_after_commit_and_after_rollback_with_the_outcome() {
    let bench = bench().await;
    let committed = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "audited", "pages": 1, "token": "hunter2" }),
            request("audit-ok"),
        )
        .await;
    assert!(committed.data().is_some());
    let events = RECORDER.audit_of("audit-ok");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].function, "shelf.book.edit");
    assert_eq!(events[0].module, "shelf");
    assert_eq!(events[0].mode, Mode::Write);
    assert_eq!(events[0].principal.level, Level::User);
    assert_eq!(events[0].params["title"], json!("audited"));
    assert_eq!(events[0].params["token"], json!(dispatcher::MASK));
    assert!(events[0].outcome.is_committed());

    let rolled_back = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "boom", "pages": 1 }),
            request("audit-boom"),
        )
        .await;
    assert_eq!(code(&rolled_back), ErrorCode::Conflict);
    let events = RECORDER.audit_of("audit-boom");
    assert_eq!(events.len(), 1);
    match &events[0].outcome {
        AuditOutcome::Failed { error } => assert_eq!(error.key, "shelf.book.error.boom"),
        AuditOutcome::Committed => panic!("a rolled back call is not committed"),
    }

    let refused = bench
        .call_with(
            principal(Level::Guest, ROOT),
            "shelf.book.edit",
            json!({ "title": "x", "pages": 1 }),
            request("audit-guest"),
        )
        .await;
    assert_eq!(code(&refused), ErrorCode::Unauthenticated);
    let events = RECORDER.audit_of("audit-guest");
    assert_eq!(events.len(), 1);
    assert!(!events[0].outcome.is_committed());

    let read = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.list",
            json!({}),
            request("audit-read"),
        )
        .await;
    assert!(read.data().is_some());
    assert!(RECORDER.audit_of("audit-read").is_empty());
    bench.finish().await;
}

#[tokio::test]
async fn secret_fields_are_masked_in_the_response_in_a_replay_and_in_the_audit_event() {
    let bench = bench().await;
    let alice = principal(Level::User, CUSTOMER);
    let options = || CallOptions {
        request_id: Some("secret-ok".into()),
        idempotency_key: Some("secret-1".into()),
        ..CallOptions::default()
    };
    let body = json!({ "title": "sealed", "pages": 1, "token": "hunter2" });
    let saved = bench
        .call_with(alice.clone(), "shelf.book.edit", body.clone(), options())
        .await;
    let data = saved.data().unwrap();
    assert_eq!(data["token"], json!(dispatcher::MASK), "{data}");
    assert!(data["id"].is_string());
    let events = RECORDER.audit_of("secret-ok");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].params["token"], json!(dispatcher::MASK));
    assert_eq!(
        bench
            .count(
                "SELECT count(*) FROM keel_idempotency WHERE response_json::text LIKE '%hunter2%'"
            )
            .await,
        0,
        "the recorded response holds no secret"
    );

    let replayed = bench
        .call_with(alice, "shelf.book.edit", body, options())
        .await;
    assert!(replayed.replay);
    assert_eq!(replayed.data().unwrap()["token"], json!(dispatcher::MASK));

    let relayed = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.relay",
            json!({ "title": "relayed", "token": "hunter3" }),
        )
        .await;
    let data = relayed.data().unwrap();
    assert_eq!(data["token"], json!(dispatcher::MASK), "{data}");
    assert!(data["id"].is_string());

    let absent = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "plain", "pages": 1 }),
        )
        .await;
    assert_eq!(absent.data().unwrap()["token"], Value::Null);
    bench.finish().await;
}

#[tokio::test]
async fn a_once_field_is_answered_to_the_call_that_produced_it_and_masked_in_the_replay() {
    let bench = bench().await;
    let alice = principal(Level::User, CUSTOMER);
    let key = || CallOptions {
        idempotency_key: Some("once-1".into()),
        ..CallOptions::default()
    };
    let body = json!({ "title": "shown", "pages": 1 });
    let first = bench
        .call_with(alice.clone(), "shelf.book.edit", body.clone(), key())
        .await;
    assert!(!first.replay);
    assert_eq!(first.data().unwrap()["receipt"], json!("rcpt-shown"));
    assert_eq!(
        bench
            .count("SELECT count(*) FROM keel_idempotency WHERE response_json::text LIKE '%rcpt-%'")
            .await,
        0,
        "the recorded response holds no value shown once"
    );
    assert_eq!(
        bench
            .count("SELECT count(*) FROM keel_idempotency WHERE response_json->>'receipt' = '***'")
            .await,
        1
    );
    let replayed = bench
        .call_with(alice.clone(), "shelf.book.edit", body, key())
        .await;
    assert!(replayed.replay);
    assert_eq!(replayed.data().unwrap()["receipt"], json!(dispatcher::MASK));
    assert_eq!(replayed.data().unwrap()["id"], first.data().unwrap()["id"]);

    let without_key = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.edit",
            json!({ "title": "plain", "pages": 1 }),
        )
        .await;
    assert_eq!(without_key.data().unwrap()["receipt"], json!("rcpt-plain"));
    bench.finish().await;
}

#[tokio::test]
async fn after_commit_runs_only_after_a_commit() {
    let bench = bench().await;
    let committed = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "later", "pages": 1 }),
            request("after-ok"),
        )
        .await;
    assert!(committed.data().is_some());
    wait_for(|| RECORDER.after_of("after-ok") == ["later"]).await;

    let rolled_back = bench
        .call_with(
            principal(Level::User, CUSTOMER),
            "shelf.book.edit",
            json!({ "title": "boom", "pages": 1 }),
            request("after-boom"),
        )
        .await;
    assert!(rolled_back.error().is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(RECORDER.after_of("after-boom").is_empty());
    bench.finish().await;
}

#[tokio::test]
async fn a_read_function_call_has_no_transaction() {
    let bench = bench().await;
    let peek = bench
        .call(Level::User, CUSTOMER, "shelf.book.peek", json!({}))
        .await;
    assert_eq!(
        peek.data().unwrap(),
        &json!({ "transaction": false, "tx_error": "keel.error.read_call_has_no_transaction" })
    );
    let meta = match peek.envelope {
        keel_types::Envelope::Ok { meta, .. } => meta,
        keel_types::Envelope::Err { .. } => panic!("ok"),
    };
    assert_eq!(meta.principal.unwrap().account.to_string(), CUSTOMER);
    bench.finish().await;
}

#[tokio::test]
async fn a_delegated_actor_is_served_in_meta_and_an_undelegated_one_is_a_kernel_error() {
    let bench = bench().await;
    let mut entered = principal(Level::User, CUSTOMER);
    let mut admin = principal(Level::Admin, ROOT);
    admin.via = Via::Su;
    entered.actor = Some(Box::new(admin.clone()));
    let served = bench
        .call_with(
            entered.clone(),
            "shelf.book.peek",
            json!({}),
            CallOptions::default(),
        )
        .await;
    let meta = match served.envelope {
        keel_types::Envelope::Ok { meta, .. } => meta,
        keel_types::Envelope::Err { error } => panic!("{error:?}"),
    };
    let actor = meta.principal.unwrap().actor.unwrap();
    assert_eq!(actor.login, admin.login);
    assert_eq!(serde_json::to_value(actor.via).unwrap(), json!("su"));

    admin.via = Via::Http;
    entered.actor = Some(Box::new(admin));
    let refused = bench
        .call_with(
            entered,
            "shelf.book.edit",
            json!({ "title": "never", "pages": 1 }),
            request("actor-http"),
        )
        .await;
    assert_eq!(code(&refused), ErrorCode::Internal);
    assert_eq!(refused.status(), 500);
    let error = refused.error().unwrap();
    assert_eq!(error.key, "keel.error.actor_not_delegated");
    assert_eq!(error.params["via"], json!("http"));
    assert_eq!(error.request_id.as_deref(), Some("actor-http"));
    assert!(bench.titles().await.is_empty());
    assert!(RECORDER.audit_of("actor-http").is_empty());
    bench.finish().await;
}

#[tokio::test]
async fn list_execution_enforces_sortable_columns_page_size_default_sort_and_totals() {
    let bench = bench().await;
    for (account, title, pages) in [
        (CUSTOMER, "gamma", 30),
        (CUSTOMER, "alpha", 10),
        (CUSTOMER, "beta", 20),
        (OTHER, "delta", 100),
    ] {
        bench
            .save(
                Level::Admin,
                ROOT,
                json!({ "title": title, "pages": pages, "account": account }),
            )
            .await;
    }
    let page = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.list",
            json!({ "pageSize": 50 }),
        )
        .await;
    let page = page.data().unwrap();
    assert_eq!(page["total"], json!(3));
    let titles: Vec<&str> = page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["alpha", "beta"]);
    assert_eq!(page["totals"], json!({ "pages": 30 }));

    let second = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.list",
            json!({ "page": 2 }),
        )
        .await;
    assert_eq!(second.data().unwrap()["data"][0]["title"], json!("gamma"));

    let unsortable = bench
        .call(
            Level::User,
            CUSTOMER,
            "shelf.book.list",
            json!({ "sort": { "key": "note", "dir": "asc" } }),
        )
        .await;
    assert_eq!(code(&unsortable), ErrorCode::Validation);
    assert_eq!(unsortable.status(), 422);
    assert_eq!(paths(&unsortable), ["sort.key"]);

    let by_pages = bench
        .call(
            Level::Reseller,
            RESELLER,
            "shelf.book.list",
            json!({ "sort": { "key": "pages", "dir": "desc" } }),
        )
        .await;
    let rows = by_pages.data().unwrap();
    assert_eq!(rows["data"][0]["title"], json!("gamma"));
    assert_eq!(rows["data"][0]["account_id"], json!(CUSTOMER));
    assert_eq!(rows["totals"], json!({ "pages": 50 }));

    let all = bench
        .call(
            Level::Admin,
            ROOT,
            "shelf.book.list",
            json!({ "search": "delta" }),
        )
        .await;
    assert_eq!(all.data().unwrap()["total"], json!(1));
    assert_eq!(all.data().unwrap()["totals"], json!({ "pages": 100 }));
    bench.finish().await;
}
