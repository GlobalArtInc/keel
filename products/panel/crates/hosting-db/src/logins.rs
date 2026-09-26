use std::collections::BTreeSet;

use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};

use crate::databases::{Batch, Deleted, MANAGE, OPTIONS, READ};
use crate::host::{Change, Effects};
use crate::names::{NAME, check_password, owner_of};
use crate::{FEATURE, database, grant, login};

const LIST: &str = "hosting.database.user.list";
const GET: &str = "hosting.database.user.get";
pub(crate) const CREATE: &str = "hosting.database.user.create";
const EDIT: &str = "hosting.database.user.edit";
const DELETE: &str = "hosting.database.user.delete";
const EXISTS: &str = "hosting.database.error.user_exists";
const FOREIGN: &str = "hosting.database.error.foreign";
const PASSWORD_MIN: u32 = 8;
const PASSWORD_MAX: u32 = 128;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct DatabaseUserCreate {
    #[schemars(length(min = 7, max = 64), pattern(NAME))]
    name: String,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Secret,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "options": OPTIONS }), "x-ui" = json!({ "control": "transfer" })))]
    databases: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct DatabaseUserEdit {
    id: Id,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password" }), "minLength" = PASSWORD_MIN, "maxLength" = PASSWORD_MAX))]
    password: Option<Secret>,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "options": OPTIONS }), "x-ui" = json!({ "control": "transfer" })))]
    databases: Vec<Id>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "camelCase")]
struct DatabaseUserView {
    #[serde(flatten)]
    login: login::Model,
    databases: Vec<Id>,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<login::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<login::Entity>()
        .ui(List::new("hosting.database.user.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (login::STATE_PENDING, Tone::Neutral),
                        (login::STATE_ACTIVE, Tone::Success),
                        (login::STATE_SUSPENDED, Tone::Warning),
                        (login::STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([
                        login::STATE_PENDING,
                        login::STATE_ACTIVE,
                        login::STATE_SUSPENDED,
                        login::STATE_ERROR,
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
            .action(Action::new("edit", ActionRole::Edit).screen(EDIT).default())
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.database.user.empty", Some("create"))
            .invalidate_on(["hosting.database.*"]))
        .default_sort("name", SortDir::Asc)
        .source(entity_source::<login::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, DatabaseUserView>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<login::Entity>()
        .handler(|call: Call<Pick>| async move {
            let db = call.db();
            let login = load(db, &call.params().id).await?;
            let databases = granted_ids(db, &login.id)
                .await?
                .iter()
                .filter_map(|id| id.parse().ok())
                .collect();
            Ok(DatabaseUserView { login, databases })
        })
}

pub(crate) fn create() -> FunctionDef {
    function::<DatabaseUserCreate, login::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<login::Entity>()
        .ui(Form::new("hosting.database.user.create.title")
            .page("main", ["name", "password"])
            .page("grants", ["databases"]))
        .handler(|call: Call<DatabaseUserCreate>| async move {
            let params = call.params();
            check_password(&params.password)?;
            let owner = owner_of(&call, &params.name).await?;
            let account = call.resolve_account(Some(owner.account)).await?;
            let tx = call.tx()?;
            refuse_taken(tx, &params.name).await?;
            let databases = own_databases(tx, &account, &params.databases).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = login::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                user_id: Set(owner.id.to_string()),
                name: Set(params.name.clone()),
                state: Set(login::STATE_PENDING.to_owned()),
                error: Set(None),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            grant_rows(tx, &row.id, databases.iter().map(|(id, _)| id.clone())).await?;
            let effects = Effects::of(&call)?;
            let id = row.id.clone();
            let password = params.password.clone();
            call.after_commit(move |ctx| async move {
                effects.create_login(&ctx, &id, &password).await
            });
            Ok(row)
        })
}

pub(crate) fn edit() -> FunctionDef {
    function::<DatabaseUserEdit, login::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<login::Entity>()
        .ui(Form::new("hosting.database.user.edit.title")
            .read(GET)
            .page("main", ["password"])
            .page("grants", ["databases"]))
        .handler(|call: Call<DatabaseUserEdit>| async move {
            let params = call.params();
            if let Some(password) = &params.password {
                check_password(password)?;
            }
            let tx = call.tx()?;
            let current = load(tx, &params.id).await?;
            let account: Id = current.account_id.parse().map_err(|_| {
                KeelError::internal("hosting.database.error.account")
                    .param("account", current.account_id.as_str())
            })?;
            let wanted = own_databases(tx, &account, &params.databases).await?;
            let held: BTreeSet<String> = granted_ids(tx, &current.id).await?.into_iter().collect();
            let wanted_ids: BTreeSet<String> = wanted.iter().map(|(id, _)| id.clone()).collect();
            let granted: Vec<String> = wanted
                .iter()
                .filter(|(id, _)| !held.contains(id))
                .map(|(_, name)| name.clone())
                .collect();
            let revoked_ids: Vec<String> = held.difference(&wanted_ids).cloned().collect();
            let revoked = names_of(tx, &revoked_ids).await?;
            if !revoked_ids.is_empty() {
                grant::Entity::delete_many()
                    .filter(grant::Column::DatabaseUserId.eq(current.id.as_str()))
                    .filter(grant::Column::DatabaseId.is_in(revoked_ids))
                    .exec(tx)
                    .await?;
            }
            grant_rows(tx, &current.id, wanted_ids.difference(&held).cloned()).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = login::ActiveModel {
                id: Set(current.id.clone()),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(tx)
            .await?;
            let change = Change {
                password: params.password.clone(),
                granted,
                revoked,
            };
            if change.password.is_some() || !change.granted.is_empty() || !change.revoked.is_empty()
            {
                let effects = Effects::of(&call)?;
                let id = row.id.clone();
                call.after_commit(move |ctx| async move {
                    effects.change_login(&ctx, &id, &change).await
                });
            }
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<login::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let names = drop_rows(call.tx()?, login::Column::Id.is_in(ids)).await?;
            let deleted = u64::try_from(names.len()).unwrap_or(u64::MAX);
            drop_after_commit(&call, names)?;
            Ok(Deleted { deleted })
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<Vec<String>> {
    let condition = filter.into_condition();
    let rows = login::Entity::find()
        .filter(condition.clone())
        .order_by_asc(login::Column::Name)
        .all(db)
        .await?;
    login::Entity::delete_many()
        .filter(condition)
        .exec(db)
        .await?;
    Ok(rows.into_iter().map(|row| row.name).collect())
}

pub(crate) fn drop_after_commit<P>(call: &Call<P>, names: Vec<String>) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    let effects = Effects::of(call)?;
    call.after_commit(move |ctx| async move { effects.drop_logins(&ctx, &names).await });
    Ok(())
}

async fn own_databases<C: ConnectionTrait>(
    db: &C,
    account: &Id,
    ids: &[Id],
) -> Result<Vec<(String, String)>> {
    let ids: BTreeSet<String> = ids.iter().map(ToString::to_string).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = database::Entity::find()
        .filter(database::Column::Id.is_in(ids.iter().cloned()))
        .filter(database::Column::AccountId.eq(account.to_string()))
        .order_by_asc(database::Column::Name)
        .all(db)
        .await?;
    if rows.len() == ids.len() {
        Ok(rows.into_iter().map(|row| (row.id, row.name)).collect())
    } else {
        Err(KeelError::validation(FOREIGN).field("databases"))
    }
}

async fn grant_rows<C: ConnectionTrait>(
    db: &C,
    login: &str,
    databases: impl IntoIterator<Item = String>,
) -> Result<()> {
    let rows: Vec<grant::ActiveModel> = databases
        .into_iter()
        .map(|database| grant::ActiveModel {
            database_user_id: Set(login.to_owned()),
            database_id: Set(database),
        })
        .collect();
    if !rows.is_empty() {
        grant::Entity::insert_many(rows).exec(db).await?;
    }
    Ok(())
}

async fn granted_ids<C: ConnectionTrait>(db: &C, login: &str) -> Result<Vec<String>> {
    Ok(grant::Entity::find()
        .filter(grant::Column::DatabaseUserId.eq(login))
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.database_id)
        .collect())
}

async fn names_of<C: ConnectionTrait>(db: &C, ids: &[String]) -> Result<Vec<String>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(database::Entity::find()
        .filter(database::Column::Id.is_in(ids.iter().cloned()))
        .order_by_asc(database::Column::Name)
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.name)
        .collect())
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
    let taken = login::Entity::find()
        .filter(login::Column::Name.eq(name))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<login::Model> {
    login::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
