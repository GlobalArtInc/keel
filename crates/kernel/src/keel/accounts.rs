use std::collections::BTreeMap;

use keel_sdk::lifecycle::AccountChange;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::prelude::DateTimeUtc;
use keel_sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel_sdk::sea_orm::sea_query::{Condition, Expr, OnConflict};
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, SqlErr, Statement,
};
use keel_sdk::system::PermissionCatalogue;
use keel_sdk::types::AccountState;
use keel_sdk::{TaskHandle, escape_like};

use super::entity::{account, principal};
use super::lifecycle::{self, STATE_CHANGED};
use super::{Pick, Saved, acl, principals};
use crate::clock::Clock;
use crate::dispatcher::guards;

pub const SYSTEM_KIND: &str = "system";
pub const RESELLER_KIND: &str = "reseller";
pub const USER_KIND: &str = "user";
const MAX_OPTIONS: u32 = 100;
const SHORT_ID_LEN: usize = 8;
pub(super) const UPSERT: &str = "keel.account.upsert";
pub(super) const SUSPEND: &str = "keel.account.suspend";
pub(super) const RESUME: &str = "keel.account.resume";
pub(super) const DELETE: &str = "keel.account.delete";
const CREATED: &str = "keel.account.created";
const SUSPENDED: &str = "keel.account.suspended";
const RESUMED: &str = "keel.account.resumed";
const DELETED: &str = "keel.account.deleted";
const SUBJECT: &str = "keel.account";
const EXTERNAL_TAKEN: &str = "keel.account.error.external_taken";
const EXTERNAL_ID: &str = "externalId";
const ROOT_IMMUTABLE: &str = "keel.account.error.root_immutable";
const OWN_ACCOUNT: &str = "keel.account.error.own_account";
const HAS_CHILDREN: &str = "keel.account.error.has_children";
const DELETING: &str = "keel.account.error.deleting";
const ROWS_REMAIN: &str = "keel.account.error.rows_remain";
const ROW_PARAM: &str = "$id";
const PARENT: &str = "parentId";
const PARENT_SCOPE: &str = "keel.account.error.parent_scope";
const REMOVE_LIMITS: &str = "DELETE FROM keel_limit WHERE account_id = $1";
const REMOVE_PRINCIPALS: &str = "DELETE FROM keel_principal WHERE account_id = $1";
const REMOVE_ACCOUNT: &str = "DELETE FROM keel_account WHERE id = $1";

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AccountCreate {
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub parent: Option<Id>,
}

#[derive(Serialize, JsonSchema)]
struct Switched {
    id: Id,
    state: AccountState,
    changed: bool,
}

#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum Deletion {
    Queued(TaskRef),
    Deleted(Deleted),
}

#[derive(Serialize, JsonSchema)]
struct Deleted {
    id: Id,
    modules: Vec<String>,
}

#[derive(Serialize)]
struct Changed<'a> {
    id: Id,
    state: &'a str,
}

#[derive(Serialize)]
struct Created<'a> {
    id: Id,
    kind: &'a str,
    state: &'a str,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct AccountUpsert {
    #[schemars(length(min = 1, max = 256), regex(pattern = principals::EXTERNAL_PATTERN))]
    pub origin: String,
    #[schemars(length(min = 1, max = 256), regex(pattern = principals::EXTERNAL_PATTERN))]
    pub external_id: String,
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub parent: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct Upserted {
    pub id: Id,
    pub created: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AccountUpdate {
    pub id: Id,
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub parent: Option<Id>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "AccountRow", rename_all = "camelCase")]
pub struct AccountRow {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub parent_id: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub path: String,
    pub state: String,
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    pub external_id: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
    pub origin: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

impl AccountRow {
    fn of(row: account::Model, labels: &BTreeMap<String, String>) -> Self {
        Self {
            label: labels
                .get(&row.id)
                .cloned()
                .unwrap_or_else(|| short_id(&row.id)),
            id: row.id,
            kind: row.kind,
            parent_id: row.parent_id,
            path: row.path,
            state: row.state,
            external_id: row.external_id,
            origin: row.origin,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

fn short_id(id: &str) -> String {
    id.get(id.len().saturating_sub(SHORT_ID_LEN)..)
        .unwrap_or(id)
        .to_owned()
}

fn may_read_logins<P>(call: &Call<P>) -> Result<bool> {
    let principal = call.principal();
    let catalogue = call.require::<dyn PermissionCatalogue>()?;
    Ok(catalogue.holds(principal, principals::READ)
        && guards::scope(principals::LIST, Mode::Read, principal).is_ok())
}

async fn labels<P>(call: &Call<P>, accounts: &[String]) -> Result<BTreeMap<String, String>> {
    if accounts.is_empty() || !may_read_logins(call)? {
        return Ok(BTreeMap::new());
    }
    let rows: Vec<(String, String)> = call
        .account_scope()
        .apply(principal::Entity::find())
        .select_only()
        .column(principal::Column::AccountId)
        .column(principal::Column::Login)
        .filter(principal::Column::AccountId.is_in(accounts.iter().map(String::as_str)))
        .filter(principal::Column::Role.eq(principals::DEFAULT_ROLE))
        .distinct_on([principal::Column::AccountId])
        .order_by_asc(principal::Column::AccountId)
        .order_by_asc(principal::Column::CreatedAt)
        .order_by_asc(principal::Column::Id)
        .into_tuple()
        .all(call.db())
        .await?;
    Ok(rows.into_iter().collect())
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<AccountRow>>("keel.account.list")
        .kind(Kind::List)
        .access(Level::User, "keel.account.read")
        .owned::<account::Entity>()
        .ui(List::new("keel.account.list.title")
            .search(["id", "kind"])
            .column(
                Column::new("label", ColumnType::Link)
                    .to("keel.account.update", [("id", "$id")])
                    .primary(),
            )
            .column(
                Column::new("kind", ColumnType::Status)
                    .map([
                        (SYSTEM_KIND, Tone::Info),
                        (RESELLER_KIND, Tone::Neutral),
                        (USER_KIND, Tone::Neutral),
                    ])
                    .sortable(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        ("active", Tone::Success),
                        ("suspended", Tone::Warning),
                        ("deleting", Tone::Danger),
                    ])
                    .filter_options(["active", "suspended", "deleting"]),
            )
            .column(
                Column::new(PARENT, ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .column(Column::new("id", ColumnType::Text).sortable())
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("origin", ColumnType::Text).level(Level::Admin))
            .action(
                Action::new("create", ActionRole::New)
                    .screen("keel.account.create")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("keel.account.update")
                    .default(),
            )
            .action(
                Action::new("suspend", ActionRole::Button)
                    .function(SUSPEND)
                    .params([("id", ROW_PARAM)])
                    .icon("pause")
                    .hidden_if("state != 'active'"),
            )
            .action(
                Action::new("resume", ActionRole::Button)
                    .function(RESUME)
                    .params([("id", ROW_PARAM)])
                    .icon("play")
                    .hidden_if("state != 'suspended'"),
            )
            .action(
                Action::new("delete", ActionRole::Button)
                    .function(DELETE)
                    .params([("id", ROW_PARAM)])
                    .icon("trash")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("label".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "suspend", "resume", "delete"])
            .empty("keel.account.empty", Some("create"))
            .invalidate_on(["keel.account.*", "keel.principal.*"])
            .tree(PARENT))
        .default_sort("id", SortDir::Asc)
        .source(Accounts)
}

struct Accounts;

#[async_trait]
impl ListSource for Accounts {
    type Row = AccountRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<AccountRow>> {
        let scope = call.account_scope();
        let mut select = scope.apply(account::Entity::find());
        match call.params().scope.get(PARENT) {
            None => {}
            Some(Value::Null) => select = select.filter(top_of(scope)),
            Some(Value::String(parent)) => {
                select = select.filter(account::Column::ParentId.eq(parent.as_str()));
            }
            Some(_) => {
                return Err(KeelError::validation(PARENT_SCOPE).field(format!("scope.{PARENT}")));
            }
        }
        let page = fetch_page(select, call, spec).await?;
        let ids: Vec<String> = page.data.iter().map(|row| row.id.clone()).collect();
        let labels = labels(call, &ids).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(|row| AccountRow::of(row, &labels))
                .collect(),
            total: page.total,
            totals: page.totals,
        })
    }
}

fn top_of(scope: &AccountScope) -> Condition {
    match scope {
        AccountScope::All => Condition::all().add(account::Column::ParentId.is_null()),
        AccountScope::Own(id) | AccountScope::Subtree(id) => {
            Condition::all().add(account::Column::Id.eq(id.to_string()))
        }
        AccountScope::Nothing => Condition::any(),
    }
}

pub fn get() -> FunctionDef {
    function::<Pick, account::Model>("keel.account.get")
        .kind(Kind::Get)
        .access(Level::User, "keel.account.read")
        .owned::<account::Entity>()
        .handler(
            |call: Call<Pick>| async move { load(call.db(), &call.params().id.to_string()).await },
        )
}

pub fn create() -> FunctionDef {
    function::<AccountCreate, Saved>("keel.account.create")
        .kind(Kind::Form)
        .access(Level::Reseller, "keel.account.manage")
        .unowned()
        .ui(Form::new("keel.account.create.title").page("main", ["kind", "parent"]))
        .handler(|call: Call<AccountCreate>| async move {
            let params = call.params();
            check_kind(&params.kind)?;
            let parent = resolve_parent(&call, params.parent).await?;
            let tx = call.tx()?;
            let parent = load(tx, &parent.to_string())
                .await
                .map_err(|error| on_field(error, "parent"))?;
            let id = Id::generate();
            let now = call.require::<dyn Clock>()?.now();
            account::ActiveModel {
                id: Set(id.to_string()),
                kind: Set(params.kind.clone()),
                parent_id: Set(Some(parent.id.clone())),
                path: Set(acl::child_path(&parent.path, &id.to_string())),
                state: Set(state_name(AccountState::Active).to_owned()),
                external_id: Set(None),
                origin: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            emit_created(&call, id, &params.kind)?;
            Ok(Saved { id })
        })
}

pub fn upsert() -> FunctionDef {
    function::<AccountUpsert, Upserted>(UPSERT)
        .kind(Kind::Action)
        .access(Level::Reseller, "keel.account.manage")
        .unowned()
        .handler(|call: Call<AccountUpsert>| async move { upsert_external(&call).await })
}

async fn upsert_external(call: &Call<AccountUpsert>) -> Result<Upserted> {
    let params = call.params();
    if let Some(kind) = &params.kind {
        check_kind(kind)?;
    }
    let tx = call.tx()?;
    if let Some(current) = by_external(tx, &params.origin, &params.external_id).await? {
        return reconcile(call, current).await;
    }
    let parent = resolve_parent(call, params.parent).await?;
    let parent = load(tx, &parent.to_string())
        .await
        .map_err(|error| on_field(error, "parent"))?;
    let id = Id::generate();
    let kind = params.kind.as_deref().unwrap_or(USER_KIND);
    let now = call.require::<dyn Clock>()?.now();
    let inserted = account::Entity::insert(account::ActiveModel {
        id: Set(id.to_string()),
        kind: Set(kind.to_owned()),
        parent_id: Set(Some(parent.id.clone())),
        path: Set(acl::child_path(&parent.path, &id.to_string())),
        state: Set(state_name(AccountState::Active).to_owned()),
        external_id: Set(Some(params.external_id.clone())),
        origin: Set(Some(params.origin.clone())),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([account::Column::Origin, account::Column::ExternalId])
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    if inserted == 0 {
        let current = by_external(tx, &params.origin, &params.external_id)
            .await?
            .ok_or_else(|| KeelError::not_found("keel.error.account_not_found"))?;
        return reconcile(call, current).await;
    }
    emit_created(call, id, kind)?;
    Ok(Upserted { id, created: true })
}

async fn by_external(tx: &Db, origin: &str, external_id: &str) -> Result<Option<account::Model>> {
    Ok(account::Entity::find()
        .filter(account::Column::Origin.eq(origin))
        .filter(account::Column::ExternalId.eq(external_id))
        .lock_exclusive()
        .one(tx)
        .await?)
}

async fn reconcile(call: &Call<AccountUpsert>, current: account::Model) -> Result<Upserted> {
    let params = call.params();
    let tx = call.tx()?;
    let id: Id = current.id.parse().map_err(|_| {
        KeelError::internal("keel.account.error.row_unknown")
            .param("id", current.id.as_str())
            .param("member", "id")
    })?;
    if !call.account_scope().includes(tx, id).await? {
        return Err(KeelError::conflict(EXTERNAL_TAKEN)
            .field(EXTERNAL_ID)
            .param("origin", params.origin.as_str())
            .param(EXTERNAL_ID, params.external_id.as_str()));
    }
    if current.state == state_name(AccountState::Deleting) {
        return Err(KeelError::conflict(DELETING).param("id", current.id.as_str()));
    }
    if let Some(kind) = params.kind.as_deref().filter(|kind| *kind != current.kind) {
        account::ActiveModel {
            id: Set(current.id.clone()),
            kind: Set(kind.to_owned()),
            updated_at: Set(call.require::<dyn Clock>()?.now()),
            ..Default::default()
        }
        .update(tx)
        .await?;
        acl::bump_subtree(tx, &current.path).await?;
    }
    Ok(Upserted { id, created: false })
}

fn emit_created<P>(call: &Call<P>, id: Id, kind: &str) -> Result<()> {
    call.emit_about(
        CREATED,
        subject(id),
        &Created {
            id,
            kind,
            state: state_name(AccountState::Active),
        },
    )
}

fn subject(id: Id) -> EventSubject {
    EventSubject {
        kind: SUBJECT.to_owned(),
        id,
        account_id: Some(id),
    }
}

pub fn update() -> FunctionDef {
    function::<AccountUpdate, Saved>("keel.account.update")
        .kind(Kind::Form)
        .access(Level::Reseller, "keel.account.manage")
        .owned::<account::Entity>()
        .ui(Form::new("keel.account.update.title")
            .read("keel.account.get")
            .page("main", ["kind", "parent"]))
        .handler(|call: Call<AccountUpdate>| async move {
            let params = call.params();
            let id = params.id;
            let tx = call.tx()?;
            let current = load(tx, &id.to_string()).await?;
            if current.parent_id.is_none() {
                return Err(KeelError::conflict(ROOT_IMMUTABLE));
            }
            let kind = match &params.kind {
                Some(kind) => {
                    check_kind(kind)?;
                    kind.clone()
                }
                None => current.kind.clone(),
            };
            let mut new_parent = None;
            if let Some(requested) = params.parent {
                let parent = resolve_parent(&call, Some(requested)).await?;
                if Some(parent.to_string()) != current.parent_id {
                    let parent = load(tx, &parent.to_string())
                        .await
                        .map_err(|error| on_field(error, "parent"))?;
                    if acl::is_within(&parent.path, &current.path) {
                        return Err(KeelError::validation("keel.account.error.parent_cycle")
                            .field("parent"));
                    }
                    new_parent = Some(parent);
                }
            }
            if kind == current.kind && new_parent.is_none() {
                return Ok(Saved { id });
            }
            let path = match &new_parent {
                Some(parent) => acl::child_path(&parent.path, &current.id),
                None => current.path.clone(),
            };
            let mut row = account::ActiveModel {
                id: Set(current.id.clone()),
                kind: Set(kind),
                updated_at: Set(call.require::<dyn Clock>()?.now()),
                ..Default::default()
            };
            if let Some(parent) = &new_parent {
                row.parent_id = Set(Some(parent.id.clone()));
                row.path = Set(path.clone());
            }
            row.update(tx).await?;
            if new_parent.is_some() {
                acl::move_descendants(tx, &current.id, &current.path, &path).await?;
            }
            acl::bump_subtree(tx, &path).await?;
            Ok(Saved { id })
        })
}

pub fn suspend() -> FunctionDef {
    switch(SUSPEND, AccountChange::Suspend)
}

pub fn resume() -> FunctionDef {
    switch(RESUME, AccountChange::Resume)
}

fn switch(name: &'static str, change: AccountChange) -> FunctionDef {
    function::<Pick, Switched>(name)
        .kind(Kind::Action)
        .access(Level::Reseller, "keel.account.manage")
        .owned::<account::Entity>()
        .handler(move |call: Call<Pick>| async move { switch_state(&call, name, change).await })
}

async fn switch_state(call: &Call<Pick>, name: &str, change: AccountChange) -> Result<Switched> {
    let (to, event) = match change {
        AccountChange::Suspend => (AccountState::Suspended, SUSPENDED),
        _ => (AccountState::Active, RESUMED),
    };
    let id = call.params().id;
    let tx = call.tx()?;
    let current = locked(tx, id).await?;
    refuse_protected(call, &current)?;
    if current.state == state_name(AccountState::Deleting) {
        return Err(KeelError::conflict(DELETING).param("id", id.to_string()));
    }
    if current.state == state_name(to) {
        return Ok(Switched {
            id,
            state: to,
            changed: false,
        });
    }
    set_state(call, tx, &current, to).await?;
    emit(call, event, id, state_name(to))?;
    let dispatcher = lifecycle::dispatcher(call, name)?;
    call.after_commit(move |context| lifecycle::after_change(dispatcher, context, id, change, to));
    Ok(Switched {
        id,
        state: to,
        changed: true,
    })
}

pub fn delete() -> FunctionDef {
    function::<Pick, Deletion>(DELETE)
        .kind(Kind::Action)
        .access(Level::Reseller, "keel.account.manage")
        .owned::<account::Entity>()
        .resumable()
        .handler(|call: Call<Pick>| async move {
            match call.task() {
                None => mark_deleting(&call).await,
                Some(task) => erase(&call, task).await,
            }
        })
}

async fn mark_deleting(call: &Call<Pick>) -> Result<Deletion> {
    let id = call.params().id;
    let tx = call.tx()?;
    let current = locked(tx, id).await?;
    refuse_protected(call, &current)?;
    let child = account::Entity::find()
        .filter(account::Column::ParentId.eq(current.id.as_str()))
        .one(tx)
        .await?;
    if child.is_some() {
        return Err(KeelError::conflict(HAS_CHILDREN).param("id", id.to_string()));
    }
    if current.state != state_name(AccountState::Deleting) {
        set_state(call, tx, &current, AccountState::Deleting).await?;
    }
    Ok(Deletion::Queued(call.spawn_self_as_task().await?))
}

async fn erase(call: &Call<Pick>, task: &TaskHandle) -> Result<Deletion> {
    let id = call.params().id;
    task.log(&format!(
        "deleting account {id} for {}",
        call.principal().login
    ))
    .await?;
    let (modules, total) = lifecycle::delete_modules(call, task, DELETE, id).await?;
    let tx = call.tx()?;
    let current = locked(tx, id).await?;
    let deleting = state_name(AccountState::Deleting);
    if current.state != deleting {
        return Err(KeelError::conflict(STATE_CHANGED)
            .param("state", current.state)
            .param("expected", deleting));
    }
    let members = principal::Entity::find()
        .filter(principal::Column::AccountId.eq(current.id.as_str()))
        .order_by_asc(principal::Column::Id)
        .all(tx)
        .await?;
    for sql in [REMOVE_LIMITS, REMOVE_PRINCIPALS, REMOVE_ACCOUNT] {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [current.id.as_str().into()],
        ))
        .await
        .map_err(rows_remain)?;
    }
    for member in &members {
        principals::emit_deleted(call, member)?;
    }
    emit(call, DELETED, id, "deleted")?;
    task.log("deleted the account row").await?;
    task.progress(total, Some(total), None).await?;
    Ok(Deletion::Deleted(Deleted { id, modules }))
}

fn rows_remain(error: DbErr) -> KeelError {
    match error.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(detail)) => {
            KeelError::conflict(ROWS_REMAIN).param("detail", detail)
        }
        _ => error.into(),
    }
}

async fn locked(tx: &Db, id: Id) -> Result<account::Model> {
    account::Entity::find_by_id(id.to_string())
        .lock_exclusive()
        .one(tx)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.account_not_found"))
}

fn refuse_protected<P>(call: &Call<P>, current: &account::Model) -> Result<()> {
    if current.parent_id.is_none() {
        return Err(KeelError::conflict(ROOT_IMMUTABLE));
    }
    if current.id == call.principal().account_id.to_string() {
        return Err(KeelError::conflict(OWN_ACCOUNT).param("id", current.id.as_str()));
    }
    Ok(())
}

async fn set_state<P>(
    call: &Call<P>,
    tx: &Db,
    current: &account::Model,
    to: AccountState,
) -> Result<()> {
    account::ActiveModel {
        id: Set(current.id.clone()),
        state: Set(state_name(to).to_owned()),
        updated_at: Set(call.require::<dyn Clock>()?.now()),
        ..Default::default()
    }
    .update(tx)
    .await?;
    Ok(())
}

fn emit<P>(call: &Call<P>, kind: &str, id: Id, state: &str) -> Result<()> {
    call.emit_about(kind, subject(id), &Changed { id, state })
}

pub fn options() -> FunctionDef {
    function::<OptionsQuery, Choices>("keel.account.options")
        .kind(Kind::Options)
        .access(Level::User, "keel.account.read")
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = query.limit.clamp(1, MAX_OPTIONS);
            let mut select = call.account_scope().apply(account::Entity::find());
            if !query.search.is_empty() {
                let pattern = format!("%{}%", escape_like(&query.search));
                select = select.filter(
                    Condition::any()
                        .add(Expr::col(account::Column::Id).ilike(pattern.as_str()))
                        .add(Expr::col(account::Column::Kind).ilike(pattern.as_str())),
                );
            }
            if let Some(cursor) = &query.cursor {
                select = select.filter(account::Column::Id.gt(cursor.as_str()));
            }
            let mut rows = select
                .order_by_asc(account::Column::Id)
                .limit(u64::from(limit) + 1)
                .all(call.db())
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
            let labels = labels(&call, &ids).await?;
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|row| {
                        let label = labels
                            .get(&row.id)
                            .cloned()
                            .unwrap_or_else(|| short_id(&row.id));
                        Choice::new(row.id, label).hint(row.kind)
                    })
                    .collect(),
                more,
            })
        })
}

pub async fn load<C: keel_sdk::sea_orm::ConnectionTrait>(
    db: &C,
    id: &str,
) -> Result<account::Model> {
    account::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.account_not_found"))
}

pub(super) fn state_name(state: AccountState) -> &'static str {
    match state {
        AccountState::Active => "active",
        AccountState::Suspended => "suspended",
        AccountState::Deleting => "deleting",
    }
}

fn check_kind(kind: &str) -> Result<()> {
    if kind == SYSTEM_KIND {
        return Err(KeelError::validation("keel.account.error.kind_reserved")
            .field("kind")
            .param("kind", kind));
    }
    Ok(())
}

async fn resolve_parent<P>(call: &Call<P>, requested: Option<Id>) -> Result<Id> {
    call.resolve_account(requested)
        .await
        .map_err(|error| on_field(error, "parent"))
}

fn on_field(error: KeelError, field: &str) -> KeelError {
    let mut moved = KeelError::with_key(error.code(), error.key()).field(field);
    for (name, value) in error.params() {
        moved = moved.param(name.clone(), value.clone());
    }
    moved.source(error)
}
