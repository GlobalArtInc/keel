use keel_sdk::lifecycle::AccountChange;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel_sdk::sea_orm::sea_query::{Condition, Expr};
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Set, SqlErr, Statement,
};
use keel_sdk::types::AccountState;
use keel_sdk::{TaskHandle, escape_like};

use super::entity::account;
use super::lifecycle::{self, STATE_CHANGED};
use super::{Pick, Saved, acl};
use crate::clock::Clock;

pub const SYSTEM_KIND: &str = "system";
pub const RESELLER_KIND: &str = "reseller";
const MAX_OPTIONS: u32 = 100;
pub(super) const SUSPEND: &str = "keel.account.suspend";
pub(super) const RESUME: &str = "keel.account.resume";
pub(super) const DELETE: &str = "keel.account.delete";
const SUSPENDED: &str = "keel.account.suspended";
const RESUMED: &str = "keel.account.resumed";
const DELETED: &str = "keel.account.deleted";
const SUBJECT: &str = "keel.account";
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

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AccountUpdate {
    pub id: Id,
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub parent: Option<Id>,
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<account::Model>>("keel.account.list")
        .kind(Kind::List)
        .access(Level::User, "keel.account.read")
        .owned::<account::Entity>()
        .ui(List::new("keel.account.list.title")
            .search(["id", "kind"])
            .column(
                Column::new("id", ColumnType::Link)
                    .to("keel.account.update", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("kind", ColumnType::Text).sortable())
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
                        items: Some("id".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "suspend", "resume", "delete"])
            .empty("keel.account.empty", Some("create"))
            .invalidate_on(["keel.account.*"])
            .tree(PARENT))
        .default_sort("id", SortDir::Asc)
        .source(Accounts)
}

struct Accounts;

#[async_trait]
impl ListSource for Accounts {
    type Row = account::Model;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<account::Model>> {
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
        fetch_page(select, call, spec).await
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
            Ok(Saved { id })
        })
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
    for sql in [REMOVE_LIMITS, REMOVE_PRINCIPALS, REMOVE_ACCOUNT] {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            [current.id.as_str().into()],
        ))
        .await
        .map_err(rows_remain)?;
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
    call.emit_about(
        kind,
        EventSubject {
            kind: SUBJECT.to_owned(),
            id,
            account_id: Some(id),
        },
        &Changed { id, state },
    )
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
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|row| Choice::new(row.id.clone(), row.id).hint(row.kind))
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
