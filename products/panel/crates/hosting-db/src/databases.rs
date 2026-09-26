use keel::prelude::*;
use keel::sdk::escape_like;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::host::{Effects, Gone};
use crate::names::{NAME, owner_of};
use crate::{FEATURE, database, grant, login};

pub(crate) const READ: &str = "hosting.database.read";
pub(crate) const MANAGE: &str = "hosting.database.manage";
const LIST: &str = "hosting.database.list";
pub(crate) const CREATE: &str = "hosting.database.create";
const DELETE: &str = "hosting.database.delete";
pub(crate) const OPTIONS: &str = "hosting.database.options";
const MAX_OPTIONS: u32 = 100;
const EXISTS: &str = "hosting.database.error.exists";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct DatabaseCreate {
    #[schemars(length(min = 7, max = 64), pattern(NAME))]
    name: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Batch {
    #[schemars(length(min = 1))]
    pub(crate) ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Deleted {
    pub(crate) deleted: u64,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<database::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<database::Entity>()
        .ui(List::new("hosting.database.list.title")
            .search(["name"])
            .column(Column::new("name", ColumnType::Text).sortable().primary())
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (database::STATE_PENDING, Tone::Neutral),
                        (database::STATE_ACTIVE, Tone::Success),
                        (database::STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([
                        database::STATE_PENDING,
                        database::STATE_ACTIVE,
                        database::STATE_ERROR,
                    ]),
            )
            .column(Column::new("error", ColumnType::Text))
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen(CREATE)
                    .icon("plus"),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["delete"])
            .empty("hosting.database.empty", Some("create"))
            .invalidate_on(["hosting.database.*"]))
        .default_sort("name", SortDir::Asc)
        .source(entity_source::<database::Entity>())
}

pub(crate) fn create() -> FunctionDef {
    function::<DatabaseCreate, database::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<database::Entity>()
        .ui(Form::new("hosting.database.create.title").page("main", ["name"]))
        .handler(|call: Call<DatabaseCreate>| async move {
            let name = call.params().name.clone();
            let owner = owner_of(&call, &name).await?;
            let account = call.resolve_account(Some(owner.account)).await?;
            let tx = call.tx()?;
            refuse_taken(tx, &name).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = database::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                user_id: Set(owner.id.to_string()),
                name: Set(name),
                state: Set(database::STATE_PENDING.to_owned()),
                error: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            let effects = Effects::of(&call)?;
            let id = row.id.clone();
            call.after_commit(move |ctx| async move { effects.create_database(&ctx, &id).await });
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<database::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let gone = drop_rows(call.tx()?, database::Column::Id.is_in(ids)).await?;
            let deleted = u64::try_from(gone.len()).unwrap_or(u64::MAX);
            drop_after_commit(&call, gone)?;
            Ok(Deleted { deleted })
        })
}

pub(crate) fn options() -> FunctionDef {
    function::<OptionsQuery, Choices>(OPTIONS)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = query.limit.clamp(1, MAX_OPTIONS);
            let mut select = call.account_scope().apply(database::Entity::find());
            if !query.search.is_empty() {
                let pattern = format!("%{}%", escape_like(&query.search));
                select = select.filter(Expr::col(database::Column::Name).ilike(pattern.as_str()));
            }
            if let Some(cursor) = &query.cursor {
                select = select.filter(database::Column::Id.gt(cursor.as_str()));
            }
            let mut rows = select
                .order_by_asc(database::Column::Id)
                .limit(u64::from(limit) + 1)
                .all(call.db())
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|row| Choice::new(row.id, row.name))
                    .collect(),
                more,
            })
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<Gone>> {
    let condition = filter.into_condition();
    let rows = database::Entity::find()
        .filter(condition.clone())
        .order_by_asc(database::Column::Name)
        .all(db)
        .await?;
    let mut gone = Vec::with_capacity(rows.len());
    for row in rows {
        let logins: Vec<String> = grant::Entity::find()
            .filter(grant::Column::DatabaseId.eq(row.id.as_str()))
            .all(db)
            .await?
            .into_iter()
            .map(|grant| grant.database_user_id)
            .collect();
        let grantees = if logins.is_empty() {
            Vec::new()
        } else {
            login::Entity::find()
                .filter(login::Column::Id.is_in(logins))
                .filter(login::Column::State.is_in([login::STATE_ACTIVE, login::STATE_SUSPENDED]))
                .order_by_asc(login::Column::Name)
                .all(db)
                .await?
                .into_iter()
                .map(|login| login.name)
                .collect()
        };
        gone.push(Gone {
            name: row.name,
            grantees,
        });
    }
    database::Entity::delete_many()
        .filter(condition)
        .exec(db)
        .await?;
    Ok(gone)
}

pub(crate) fn drop_after_commit<P>(call: &Call<P>, gone: Vec<Gone>) -> Result<()> {
    if gone.is_empty() {
        return Ok(());
    }
    let effects = Effects::of(call)?;
    call.after_commit(move |ctx| async move { effects.drop_databases(&ctx, &gone).await });
    Ok(())
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
    let taken = database::Entity::find()
        .filter(database::Column::Name.eq(name))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}
