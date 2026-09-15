use crate::support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use keel_sdk::prelude::*;
use keel_sdk::runtime::{CallBuilder, CallHandle, Ownership};
use keel_sdk::sea_orm::{ActiveModelTrait, Set, TransactionTrait};
use keel_sdk::types::ErrorCode;

use crate::support::{CUSTOMER, OTHER, RESELLER, ROOT};

mod item {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "memo_item")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
        pub weight: i32,
        pub pinned: bool,
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
pub struct ItemEdit {
    pub id: Option<Id>,
    pub title: String,
    pub weight: i32,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct Saved {
    pub id: Id,
}

pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str);
}

struct Counter(AtomicUsize);

impl Notifier for Counter {
    fn notify(&self, _: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/sdk/memo/migrations").table("keel_migration_memo");

pub static MODULE: ModuleDef = ModuleDef {
    id: "memo",
    version: env!("CARGO_PKG_VERSION"),
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(list())
        .function(edit())
        .menu(
            MenuNode::new("memo/items", "memo.item.list")
                .icon("book-open")
                .order(10),
        )
        .i18n("en", include_str!("memo/i18n/en.json"))
        .permission("memo.item.read", &[Level::User])
        .permission("memo.item.manage", &[Level::User])
        .require::<dyn Notifier>();
    Ok(())
}

fn list() -> FunctionDef {
    function::<ListQuery, Page<item::Model>>("memo.item.list")
        .kind(Kind::List)
        .access(Level::User, "memo.item.read")
        .owned::<item::Entity>()
        .ui(List::new("memo.item.list.title")
            .search(["title"])
            .column(Column::new("title", ColumnType::Text).sortable().primary())
            .column(Column::new("weight", ColumnType::Number).sortable())
            .column(Column::new("pinned", ColumnType::Toggle))
            .empty("memo.item.empty", None))
        .default_sort("title", SortDir::Asc)
        .max_page_size(2)
        .source(entity_source::<item::Entity>())
}

fn edit() -> FunctionDef {
    function::<ItemEdit, Saved>("memo.item.edit")
        .kind(Kind::Form)
        .access(Level::User, "memo.item.manage")
        .owned::<item::Entity>()
        .ui(Form::new("memo.item.edit.title")
            .read("memo.item.get")
            .page("main", ["title", "weight", "account"]))
        .handler(|call: Call<ItemEdit>| async move {
            let account = call.resolve_account(call.params().account).await?;
            let id = call.params().id.unwrap_or_else(Id::generate);
            let row = item::ActiveModel {
                id: Set(id.to_string()),
                account_id: Set(account.to_string()),
                title: Set(call.params().title.clone()),
                weight: Set(call.params().weight),
                pinned: Set(false),
            };
            let saved = match call.params().id {
                Some(_) => row.update(call.tx()?).await?,
                None => row.insert(call.tx()?).await.map_err(|error| {
                    KeelError::conflict("memo.item.error.exists")
                        .field("title")
                        .source(error)
                })?,
            };
            call.emit("memo.item.saved", &saved)?;
            let notifier = call.require::<dyn Notifier>()?;
            call.after_commit(move |ctx| async move {
                assert_eq!(ctx.principal().level, Level::User);
                notifier.notify(&saved.title);
                Ok(())
            });
            Ok(Saved { id })
        })
}

fn registrations() -> keel_sdk::runtime::Registrations {
    let mut registrar = Registrar::new(MODULE.id);
    (MODULE.init)(&mut registrar).expect("init registers");
    registrar
        .into_registrations()
        .expect("registrations are valid")
}

#[test]
fn the_module_defines_one_read_and_one_write_function() {
    let registrations = registrations();
    assert_eq!(MODULE.sdk, keel_sdk::VERSION);
    assert_eq!(
        MODULE.migrations.map(|migrations| migrations.ledger()),
        Some("keel_migration_memo")
    );
    let modes: Vec<(&str, Mode)> = registrations
        .functions
        .iter()
        .map(|def| (def.name(), def.mode()))
        .collect();
    assert_eq!(
        modes,
        [
            ("memo.item.list", Mode::Read),
            ("memo.item.edit", Mode::Write)
        ]
    );
    assert_eq!(registrations.menu[0].func(), "memo.item.list");
    assert_eq!(registrations.permissions.len(), 2);
    assert_eq!(
        registrations.requires[0].name(),
        std::any::type_name::<dyn Notifier>()
    );
    for def in &registrations.functions {
        assert_eq!(
            def.ownership(),
            &Ownership::Owned(keel_sdk::runtime::OwnedDef {
                table: "memo_item".into(),
                account_column: "account_id".into(),
                primary_key: vec!["id".into()],
            })
        );
        assert_eq!(def.key(), Some("id"));
        let descriptor = def.descriptor().expect("descriptor compiles");
        assert_eq!(descriptor.module, "memo");
    }
}

async fn migrated() -> support::TestDatabase {
    let database = support::create().await.with_accounts().await;
    let applied = MIGRATIONS
        .apply(&database.db)
        .await
        .expect("migrations apply");
    assert_eq!(applied, ["CreateMemoItem1757600000000"]);
    let again = MIGRATIONS
        .apply(&database.db)
        .await
        .expect("migrations are idempotent");
    assert!(again.is_empty());
    database
}

fn services() -> (Arc<keel_sdk::runtime::Services>, Arc<Counter>) {
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    let mut services = keel_sdk::runtime::Services::new();
    services.provide::<dyn Notifier>(counter.clone());
    (Arc::new(services), counter)
}

async fn save_with(
    database: &support::TestDatabase,
    level: Level,
    account: &str,
    params: Value,
    services: Arc<keel_sdk::runtime::Services>,
) -> (Result<Value>, CallHandle) {
    let transaction = database.db.begin().await.expect("begin");
    let handle = CallBuilder::new(support::principal(level, account), "req")
        .services(services)
        .write(transaction);
    let result = (edit().handler())(handle.call(params)).await;
    (result, handle)
}

async fn save(
    database: &support::TestDatabase,
    level: Level,
    account: &str,
    params: Value,
) -> (Result<Value>, CallHandle) {
    save_with(database, level, account, params, services().0).await
}

async fn commit(handle: CallHandle) {
    let transaction = handle
        .finish()
        .expect("nothing else holds the call")
        .expect("a write call carries its transaction");
    transaction.commit().await.expect("commit");
}

async fn list_as(
    database: &support::TestDatabase,
    level: Level,
    account: &str,
    query: Value,
) -> Value {
    let handle =
        CallBuilder::new(support::principal(level, account), "req").read(database.db.clone());
    (list().handler())(handle.call(query))
        .await
        .expect("list answers")
}

fn titles(page: &Value) -> Vec<&str> {
    page["data"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["title"].as_str().expect("title"))
        .collect()
}

#[tokio::test]
async fn a_write_call_saves_inside_its_transaction_and_reports_events_and_hooks() {
    let database = migrated().await;
    let (services, counter) = services();
    let (result, handle) = save_with(
        &database,
        Level::User,
        CUSTOMER,
        json!({ "title": "first", "weight": 3 }),
        services,
    )
    .await;
    let saved = result.expect("the handler saves");
    let id: Id = saved["id"].as_str().expect("id").parse().expect("ulid");
    let events = handle.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "memo.item.saved");
    assert_eq!(events[0].data["id"], json!(id.to_string()));
    assert_eq!(events[0].data["account_id"], json!(CUSTOMER));
    let hooks = handle.take_after_commit();
    assert_eq!(hooks.len(), 1);
    let context = handle.after_context();
    assert_eq!(context.request_id(), "req");
    assert_eq!(context.principal().level, Level::User);

    let before_commit = list_as(&database, Level::Admin, ROOT, json!({})).await;
    assert_eq!(before_commit["total"], json!(0));
    commit(handle).await;
    assert_eq!(counter.0.load(Ordering::SeqCst), 0);
    for hook in hooks {
        hook(context.clone()).await.expect("after commit hook runs");
    }
    assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    let after_commit = list_as(&database, Level::Admin, ROOT, json!({})).await;
    assert_eq!(titles(&after_commit), ["first"]);
    database.drop().await;
}

#[tokio::test]
async fn a_handler_error_keeps_its_field_and_the_transaction_rolls_back() {
    let database = migrated().await;
    let (created, handle) = save(
        &database,
        Level::User,
        CUSTOMER,
        json!({ "title": "dup", "weight": 1 }),
    )
    .await;
    created.expect("the first insert succeeds");
    commit(handle).await;

    let (duplicate, handle) = save(
        &database,
        Level::User,
        CUSTOMER,
        json!({ "title": "dup", "weight": 2 }),
    )
    .await;
    let error = duplicate.expect_err("the second insert conflicts");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.field_path(), Some("title"));
    let wire = error.to_call_error();
    assert_eq!(wire.code, ErrorCode::Validation);
    assert_eq!(wire.fields[0].path, "title");
    assert_eq!(
        wire.fields[0].key.as_deref(),
        Some("memo.item.error.exists")
    );
    assert!(std::error::Error::source(&error).is_some());
    let transaction = handle.finish().expect("free").expect("write mode");
    transaction.rollback().await.expect("rollback");

    let (missing, handle) = save(
        &database,
        Level::User,
        CUSTOMER,
        json!({ "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "title": "renamed", "weight": 1 }),
    )
    .await;
    assert_eq!(
        missing.expect_err("updating a missing row").code(),
        ErrorCode::NotFound
    );
    drop(handle);

    let page = list_as(&database, Level::User, CUSTOMER, json!({})).await;
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["weight"], json!(1));
    database.drop().await;
}

#[tokio::test]
async fn resolve_account_accepts_only_accounts_inside_the_scope() {
    let database = migrated().await;
    let (foreign, handle) = save(
        &database,
        Level::User,
        CUSTOMER,
        json!({ "title": "x", "weight": 1, "account": OTHER }),
    )
    .await;
    let error = foreign.expect_err("a user cannot choose another account");
    assert_eq!(error.code(), ErrorCode::NotFound);
    assert_eq!(error.field_path(), Some("account"));
    assert_eq!(error.to_call_error().fields[0].path, "account");
    drop(handle);

    let (outside, handle) = save(
        &database,
        Level::Reseller,
        RESELLER,
        json!({ "title": "x", "weight": 1, "account": OTHER }),
    )
    .await;
    assert_eq!(
        outside.expect_err("outside the subtree").code(),
        ErrorCode::NotFound
    );
    drop(handle);

    let (inside, handle) = save(
        &database,
        Level::Reseller,
        RESELLER,
        json!({ "title": "for the customer", "weight": 1, "account": CUSTOMER }),
    )
    .await;
    inside.expect("inside the subtree");
    commit(handle).await;

    let (missing, handle) = save(
        &database,
        Level::Admin,
        ROOT,
        json!({ "title": "x", "weight": 1, "account": "01J8A0000000000000000000ZZ" }),
    )
    .await;
    assert_eq!(
        missing.expect_err("an unknown account").code(),
        ErrorCode::NotFound
    );
    drop(handle);

    let page = list_as(&database, Level::User, CUSTOMER, json!({})).await;
    assert_eq!(titles(&page), ["for the customer"]);
    database.drop().await;
}

#[tokio::test]
async fn the_entity_source_scopes_searches_sorts_and_pages_by_the_caller() {
    let database = migrated().await;
    for (account, title, weight) in [
        (CUSTOMER, "customer alpha", 3),
        (CUSTOMER, "customer beta", 1),
        (CUSTOMER, "customer gamma", 2),
        (RESELLER, "reseller own", 5),
        (OTHER, "other one", 9),
    ] {
        let (result, handle) = save(
            &database,
            Level::Admin,
            ROOT,
            json!({ "title": title, "weight": weight, "account": account }),
        )
        .await;
        result.expect("seeded");
        commit(handle).await;
    }

    let user = list_as(&database, Level::User, CUSTOMER, json!({ "pageSize": 50 })).await;
    assert_eq!(user["total"], json!(3));
    assert_eq!(titles(&user), ["customer alpha", "customer beta"]);
    let second = list_as(&database, Level::User, CUSTOMER, json!({ "page": 2 })).await;
    assert_eq!(titles(&second), ["customer gamma"]);

    let reseller = list_as(
        &database,
        Level::Reseller,
        RESELLER,
        json!({ "sort": { "key": "weight", "dir": "desc" } }),
    )
    .await;
    assert_eq!(reseller["total"], json!(4));
    assert_eq!(titles(&reseller), ["reseller own", "customer alpha"]);

    let other = list_as(&database, Level::User, OTHER, json!({})).await;
    assert_eq!(titles(&other), ["other one"]);

    let admin = list_as(&database, Level::Admin, ROOT, json!({ "search": "ALPHA" })).await;
    assert_eq!(titles(&admin), ["customer alpha"]);
    let filtered = list_as(
        &database,
        Level::Admin,
        ROOT,
        json!({ "filters": [ { "key": "weight", "op": "gte", "value": "5" } ] }),
    )
    .await;
    assert_eq!(filtered["total"], json!(2));
    assert_eq!(titles(&filtered), ["other one", "reseller own"]);

    let handle =
        CallBuilder::new(support::principal(Level::Guest, ROOT), "req").read(database.db.clone());
    let guest = (list().handler())(handle.call(json!({})))
        .await
        .expect("a guest list is empty");
    assert_eq!(guest["total"], json!(0));

    let handle = CallBuilder::new(support::principal(Level::User, CUSTOMER), "req")
        .read(database.db.clone());
    let unsortable =
        (list().handler())(handle.call(json!({ "sort": { "key": "account_id", "dir": "asc" } })))
            .await
            .expect_err("account_id is not a sortable column");
    assert_eq!(unsortable.code(), ErrorCode::Validation);
    database.drop().await;
}

#[tokio::test]
async fn a_read_call_has_neither_transaction_nor_outbox() {
    let database = support::create().await;
    let handle = CallBuilder::new(support::principal(Level::User, CUSTOMER), "req")
        .read(database.db.clone());
    let call = handle.call(json!({}));
    assert_eq!(call.mode(), Mode::Read);
    assert!(!call.db().is_transaction());
    assert_eq!(
        call.tx().expect_err("no transaction").key(),
        "keel.error.read_call_has_no_transaction"
    );
    assert_eq!(
        call.emit("memo.item.saved", &json!({}))
            .expect_err("no outbox")
            .key(),
        "keel.error.read_call_has_no_outbox"
    );
    assert_eq!(
        call.invoke("memo.item.list", json!({}))
            .await
            .expect_err("no invoker")
            .code(),
        ErrorCode::Internal
    );
    let missing = call.require::<dyn Notifier>().err().expect("not provided");
    assert_eq!(missing.key(), "keel.error.service_missing");
    drop(call);
    assert!(handle.finish().expect("free").is_none());
    database.drop().await;
}
