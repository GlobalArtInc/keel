use keel_sdk::escape_like;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel_sdk::sea_orm::sea_query::{Condition, Expr};
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use keel_sdk::types::AccountState;

use super::acl;
use super::entity::account;
use super::{Pick, Saved};
use crate::clock::Clock;

pub const SYSTEM_KIND: &str = "system";
pub const RESELLER_KIND: &str = "reseller";
const MAX_OPTIONS: u32 = 100;

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct AccountCreate {
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub parent: Option<Id>,
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
                Column::new("parentId", ColumnType::Text)
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
            .row_actions(["edit"])
            .empty("keel.account.empty", Some("create"))
            .invalidate_on(["keel.account.*"]))
        .default_sort("id", SortDir::Asc)
        .source(entity_source::<account::Entity>())
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
                state: Set(state(AccountState::Active)),
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
                return Err(KeelError::conflict("keel.account.error.root_immutable"));
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

fn state(state: AccountState) -> String {
    match state {
        AccountState::Active => "active",
        AccountState::Suspended => "suspended",
        AccountState::Deleting => "deleting",
    }
    .to_owned()
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
