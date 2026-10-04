use chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict};
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, DbErr, EntityTrait, QueryFilter, QuerySelect, Set, SqlErr,
};
use keel_sdk::serde_json;

use super::accounts::{self, AccountUpsert, RESELLER_KIND, SYSTEM_KIND, Upserted};
use super::entity::{account, principal};
use super::{Pick, Saved};
use crate::clock::Clock;

pub const DEFAULT_ROLE: &str = "owner";
pub const DEFAULT_LANG: &str = "en";
pub const EXTERNAL_PATTERN: &str = r"^[!#-+\--:<-\[\]-~]+$";
pub(super) const UPSERT: &str = "keel.principal.upsert";
const CREATED: &str = "keel.principal.created";
const RENAMED: &str = "keel.principal.renamed";
const DELETED: &str = "keel.principal.deleted";
const SUBJECT: &str = "keel.principal";
const EXTERNAL_TAKEN: &str = "keel.principal.error.external_taken";
const EXTERNAL_ID: &str = "externalId";
const ACCOUNT: &str = "account";

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PrincipalCreate {
    #[schemars(length(min = 1, max = 64))]
    pub login: String,
    pub level: Level,
    pub role: Option<String>,
    pub lang: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct PrincipalUpdate {
    pub id: Id,
    #[schemars(length(min = 1, max = 64))]
    pub login: Option<String>,
    pub level: Option<Level>,
    pub role: Option<String>,
    pub lang: Option<String>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct PrincipalUpsert {
    #[schemars(length(min = 1, max = 256), regex(pattern = EXTERNAL_PATTERN))]
    pub origin: String,
    #[schemars(length(min = 1, max = 256), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    #[schemars(length(min = 1, max = 64))]
    pub login: String,
    pub level: Level,
    pub role: Option<String>,
    pub lang: Option<String>,
    pub account: AccountUpsert,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
struct PrincipalUpserted {
    id: Id,
    account_id: Id,
    created: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Created<'a> {
    id: Id,
    login: &'a str,
    account_id: Id,
}

#[derive(Serialize)]
struct Renamed<'a> {
    id: Id,
    login: &'a str,
}

#[derive(Serialize)]
struct Deleted {
    id: Id,
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<principal::Model>>("keel.principal.list")
        .kind(Kind::List)
        .access(Level::User, "keel.principal.read")
        .owned::<principal::Entity>()
        .ui(List::new("keel.principal.list.title")
            .search(["login"])
            .column(
                Column::new("login", ColumnType::Link)
                    .to("keel.principal.update", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("level", ColumnType::Text)
                    .sortable()
                    .filter_options(["service", "user", "reseller", "admin", "owner"]),
            )
            .column(Column::new("role", ColumnType::Text).sortable())
            .column(
                Column::new("state", ColumnType::Status)
                    .map([("active", Tone::Success), ("suspended", Tone::Warning)])
                    .filter_options(["active", "suspended"]),
            )
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .column(Column::new("lang", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("origin", ColumnType::Text).level(Level::Admin))
            .action(
                Action::new("create", ActionRole::New)
                    .screen("keel.principal.create")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("keel.principal.update")
                    .default(),
            )
            .row_actions(["edit"])
            .empty("keel.principal.empty", Some("create"))
            .invalidate_on(["keel.principal.*"]))
        .default_sort("login", SortDir::Asc)
        .source(entity_source::<principal::Entity>())
}

pub fn get() -> FunctionDef {
    function::<Pick, principal::Model>("keel.principal.get")
        .kind(Kind::Get)
        .access(Level::User, "keel.principal.read")
        .owned::<principal::Entity>()
        .handler(
            |call: Call<Pick>| async move { load(call.db(), &call.params().id.to_string()).await },
        )
}

pub fn create() -> FunctionDef {
    function::<PrincipalCreate, Saved>("keel.principal.create")
        .kind(Kind::Form)
        .access(Level::User, "keel.principal.manage")
        .unowned()
        .ui(Form::new("keel.principal.create.title")
            .page("main", ["login", "level", "role", "lang", "account"]))
        .handler(|call: Call<PrincipalCreate>| async move {
            let params = call.params();
            let account_id = call.resolve_account(params.account).await?;
            let tx = call.tx()?;
            let account = accounts::load(tx, &account_id.to_string()).await?;
            check_level(call.principal(), params.level, &account)?;
            let now = call.require::<dyn Clock>()?.now();
            let id = insert(
                tx,
                NewPrincipal {
                    account_id: account.id,
                    login: params.login.clone(),
                    level: params.level,
                    role: params
                        .role
                        .clone()
                        .unwrap_or_else(|| DEFAULT_ROLE.to_owned()),
                    lang: params
                        .lang
                        .clone()
                        .unwrap_or_else(|| DEFAULT_LANG.to_owned()),
                },
                now,
            )
            .await?;
            emit_created(&call, id, &params.login, account_id)?;
            Ok(Saved { id })
        })
}

pub(super) struct NewPrincipal {
    pub(super) account_id: String,
    pub(super) login: String,
    pub(super) level: Level,
    pub(super) role: String,
    pub(super) lang: String,
}

fn row(id: Id, principal: NewPrincipal, now: DateTime<Utc>) -> principal::ActiveModel {
    principal::ActiveModel {
        id: Set(id.to_string()),
        account_id: Set(principal.account_id),
        login: Set(principal.login),
        level: Set(principal.level.as_str().to_owned()),
        role: Set(principal.role),
        state: Set("active".to_owned()),
        lang: Set(principal.lang),
        external_id: Set(None),
        origin: Set(None),
        acl_gen: Set(0),
        created_at: Set(now),
        updated_at: Set(now),
    }
}

pub(super) async fn insert(db: &Db, principal: NewPrincipal, now: DateTime<Utc>) -> Result<Id> {
    let id = Id::generate();
    row(id, principal, now)
        .insert(db)
        .await
        .map_err(login_taken)?;
    Ok(id)
}

pub fn update() -> FunctionDef {
    function::<PrincipalUpdate, Saved>("keel.principal.update")
        .kind(Kind::Form)
        .access(Level::User, "keel.principal.manage")
        .owned::<principal::Entity>()
        .ui(Form::new("keel.principal.update.title")
            .read("keel.principal.get")
            .page("main", ["login", "level", "role", "lang", "account"]))
        .handler(|call: Call<PrincipalUpdate>| async move {
            let params = call.params();
            let id = params.id;
            let current = load(call.tx()?, &id.to_string()).await?;
            refuse_above(&call, &current)?;
            let account_id = match params.account {
                Some(requested) => Some(call.resolve_account(Some(requested)).await?),
                None => None,
            };
            change(
                &call,
                &current,
                Change {
                    login: params.login.as_deref(),
                    level: params.level,
                    role: params.role.as_deref(),
                    lang: params.lang.as_deref(),
                    account_id,
                },
            )
            .await?;
            Ok(Saved { id })
        })
}

pub fn upsert() -> FunctionDef {
    function::<PrincipalUpsert, PrincipalUpserted>(UPSERT)
        .kind(Kind::Action)
        .access(Level::Reseller, "keel.principal.manage")
        .unowned()
        .handler(|call: Call<PrincipalUpsert>| async move { upsert_external(&call).await })
}

async fn upsert_external(call: &Call<PrincipalUpsert>) -> Result<PrincipalUpserted> {
    let params = call.params();
    let upserted = call
        .invoke(accounts::UPSERT, serde_json::to_value(&params.account)?)
        .await
        .map_err(under_account)?;
    let account_id = serde_json::from_value::<Upserted>(upserted)?.id;
    let tx = call.tx()?;
    if let Some(current) = by_external(tx, &params.origin, &params.external_id).await? {
        return reconcile(call, &current, account_id).await;
    }
    let account = accounts::load(tx, &account_id.to_string()).await?;
    check_level(call.principal(), params.level, &account)?;
    let id = Id::generate();
    let mut new = row(
        id,
        NewPrincipal {
            account_id: account.id,
            login: params.login.clone(),
            level: params.level,
            role: params
                .role
                .clone()
                .unwrap_or_else(|| DEFAULT_ROLE.to_owned()),
            lang: params
                .lang
                .clone()
                .unwrap_or_else(|| DEFAULT_LANG.to_owned()),
        },
        call.require::<dyn Clock>()?.now(),
    );
    new.origin = Set(Some(params.origin.clone()));
    new.external_id = Set(Some(params.external_id.clone()));
    let inserted = principal::Entity::insert(new)
        .on_conflict(
            OnConflict::columns([principal::Column::Origin, principal::Column::ExternalId])
                .do_nothing()
                .to_owned(),
        )
        .exec_without_returning(tx)
        .await
        .map_err(login_taken)?;
    if inserted == 0 {
        let current = by_external(tx, &params.origin, &params.external_id)
            .await?
            .ok_or_else(|| KeelError::not_found("keel.error.principal_not_found"))?;
        return reconcile(call, &current, account_id).await;
    }
    emit_created(call, id, &params.login, account_id)?;
    Ok(PrincipalUpserted {
        id,
        account_id,
        created: true,
    })
}

async fn by_external(tx: &Db, origin: &str, external_id: &str) -> Result<Option<principal::Model>> {
    Ok(principal::Entity::find()
        .filter(principal::Column::Origin.eq(origin))
        .filter(principal::Column::ExternalId.eq(external_id))
        .lock_exclusive()
        .one(tx)
        .await?)
}

async fn reconcile(
    call: &Call<PrincipalUpsert>,
    current: &principal::Model,
    account_id: Id,
) -> Result<PrincipalUpserted> {
    let params = call.params();
    let (id, holder) = ids_of(current)?;
    if !call.account_scope().includes(call.tx()?, holder).await? {
        return Err(KeelError::conflict(EXTERNAL_TAKEN)
            .field(EXTERNAL_ID)
            .param("origin", params.origin.as_str())
            .param(EXTERNAL_ID, params.external_id.as_str()));
    }
    refuse_above(call, current)?;
    change(
        call,
        current,
        Change {
            login: Some(&params.login),
            level: Some(params.level),
            role: params.role.as_deref(),
            lang: params.lang.as_deref(),
            account_id: Some(account_id),
        },
    )
    .await?;
    Ok(PrincipalUpserted {
        id,
        account_id,
        created: false,
    })
}

fn under_account(error: KeelError) -> KeelError {
    let mut wire = error.to_call_error();
    if wire.fields.is_empty() {
        return error;
    }
    for field in &mut wire.fields {
        field.path = format!("{ACCOUNT}.{}", field.path);
    }
    KeelError::from(wire)
}

struct Change<'a> {
    login: Option<&'a str>,
    level: Option<Level>,
    role: Option<&'a str>,
    lang: Option<&'a str>,
    account_id: Option<Id>,
}

fn refuse_above<P>(call: &Call<P>, current: &principal::Model) -> Result<()> {
    let current_level = stored_level(current)?;
    if current_level > call.principal().level {
        return Err(
            KeelError::forbidden("keel.principal.error.level_above_caller")
                .param("level", current_level.as_str()),
        );
    }
    Ok(())
}

async fn change<P>(call: &Call<P>, current: &principal::Model, change: Change<'_>) -> Result<()> {
    let tx = call.tx()?;
    let (id, _) = ids_of(current)?;
    let current_level = stored_level(current)?;
    let level = change.level.unwrap_or(current_level);
    let account_id = change
        .account_id
        .map_or_else(|| current.account_id.clone(), |account| account.to_string());
    if level != current_level || account_id != current.account_id {
        let account = accounts::load(tx, &account_id).await?;
        check_level(call.principal(), level, &account)?;
    }
    let role = change.role.unwrap_or(&current.role);
    let rights_changed =
        level != current_level || role != current.role || account_id != current.account_id;
    let mut update = principal::Entity::update_many()
        .col_expr(principal::Column::Level, Expr::value(level.as_str()))
        .col_expr(principal::Column::Role, Expr::value(role))
        .col_expr(
            principal::Column::AccountId,
            Expr::value(account_id.as_str()),
        )
        .col_expr(
            principal::Column::UpdatedAt,
            Expr::value(call.require::<dyn Clock>()?.now()),
        )
        .filter(principal::Column::Id.eq(current.id.as_str()));
    if let Some(login) = change.login {
        update = update.col_expr(principal::Column::Login, Expr::value(login));
    }
    if let Some(lang) = change.lang {
        update = update.col_expr(principal::Column::Lang, Expr::value(lang));
    }
    if rights_changed {
        update = update.col_expr(
            principal::Column::AclGen,
            Expr::col(principal::Column::AclGen).add(1),
        );
    }
    update.exec(tx).await.map_err(login_taken)?;
    if let Some(login) = change.login.filter(|login| *login != current.login) {
        let account = account_id.parse().map_err(|_| account_unknown(current))?;
        call.emit_about(RENAMED, subject(id, account), &Renamed { id, login })?;
    }
    Ok(())
}

fn emit_created<P>(call: &Call<P>, id: Id, login: &str, account_id: Id) -> Result<()> {
    call.emit_about(
        CREATED,
        subject(id, account_id),
        &Created {
            id,
            login,
            account_id,
        },
    )
}

pub(super) fn emit_deleted<P>(call: &Call<P>, member: &principal::Model) -> Result<()> {
    let (id, account) = ids_of(member)?;
    call.emit_about(DELETED, subject(id, account), &Deleted { id })
}

fn subject(id: Id, account: Id) -> EventSubject {
    EventSubject {
        kind: SUBJECT.to_owned(),
        id,
        account_id: Some(account),
    }
}

fn ids_of(row: &principal::Model) -> Result<(Id, Id)> {
    let id = row.id.parse().map_err(|_| {
        KeelError::internal("keel.principal.error.id_unknown").param("id", row.id.as_str())
    })?;
    let account = row.account_id.parse().map_err(|_| account_unknown(row))?;
    Ok((id, account))
}

fn account_unknown(row: &principal::Model) -> KeelError {
    KeelError::internal("keel.principal.error.account_unknown")
        .param("id", row.id.as_str())
        .param(ACCOUNT, row.account_id.as_str())
}

async fn load<C: keel_sdk::sea_orm::ConnectionTrait>(db: &C, id: &str) -> Result<principal::Model> {
    principal::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}

fn stored_level(row: &principal::Model) -> Result<Level> {
    row.level.parse().map_err(|_| {
        KeelError::internal("keel.principal.error.level_unknown")
            .param("id", row.id.as_str())
            .param("level", row.level.as_str())
    })
}

fn check_level(caller: &Principal, level: Level, account: &account::Model) -> Result<()> {
    if level == Level::Guest {
        return Err(KeelError::validation("keel.principal.error.level_guest").field("level"));
    }
    if level > caller.level {
        return Err(
            KeelError::validation("keel.principal.error.level_above_caller")
                .field("level")
                .param("level", level.as_str()),
        );
    }
    let fits = match level {
        Level::Admin | Level::Owner => account.kind == SYSTEM_KIND,
        Level::Reseller => account.kind == RESELLER_KIND,
        Level::Guest | Level::Service | Level::User => true,
    };
    if !fits {
        return Err(KeelError::validation("keel.principal.error.level_account")
            .field("level")
            .param("level", level.as_str())
            .param("kind", account.kind.as_str()));
    }
    Ok(())
}

fn login_taken(error: DbErr) -> KeelError {
    match error.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(_)) => {
            KeelError::conflict("keel.principal.error.login_exists")
                .field("login")
                .source(error)
        }
        _ => error.into(),
    }
}
