use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, DbErr, EntityTrait, QueryFilter, Set, SqlErr,
};

use super::accounts::{self, RESELLER_KIND, SYSTEM_KIND};
use super::entity::{account, principal};
use super::{Pick, Saved};
use crate::clock::Clock;

pub const DEFAULT_ROLE: &str = "owner";
pub const DEFAULT_LANG: &str = "en";

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
                Column::new("account", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .column(Column::new("lang", ColumnType::Text))
            .column(Column::new("created_at", ColumnType::Datetime).sortable())
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
            let id = Id::generate();
            let now = call.require::<dyn Clock>()?.now();
            principal::ActiveModel {
                id: Set(id.to_string()),
                account: Set(account.id),
                login: Set(params.login.clone()),
                level: Set(params.level.as_str().to_owned()),
                role: Set(params
                    .role
                    .clone()
                    .unwrap_or_else(|| DEFAULT_ROLE.to_owned())),
                state: Set("active".to_owned()),
                lang: Set(params
                    .lang
                    .clone()
                    .unwrap_or_else(|| DEFAULT_LANG.to_owned())),
                external_id: Set(None),
                origin: Set(None),
                acl_gen: Set(0),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await
            .map_err(login_taken)?;
            Ok(Saved { id })
        })
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
            let tx = call.tx()?;
            let current = load(tx, &id.to_string()).await?;
            let current_level = stored_level(&current)?;
            if current_level > call.principal().level {
                return Err(
                    KeelError::forbidden("keel.principal.error.level_above_caller")
                        .param("level", current_level.as_str()),
                );
            }
            let level = params.level.unwrap_or(current_level);
            let account_id = match params.account {
                Some(requested) => call.resolve_account(Some(requested)).await?.to_string(),
                None => current.account.clone(),
            };
            if level != current_level || account_id != current.account {
                let account = accounts::load(tx, &account_id).await?;
                check_level(call.principal(), level, &account)?;
            }
            let role = params.role.clone().unwrap_or_else(|| current.role.clone());
            let rights_changed =
                level != current_level || role != current.role || account_id != current.account;
            let mut update = principal::Entity::update_many()
                .col_expr(principal::Column::Level, Expr::value(level.as_str()))
                .col_expr(principal::Column::Role, Expr::value(role))
                .col_expr(principal::Column::Account, Expr::value(account_id))
                .col_expr(
                    principal::Column::UpdatedAt,
                    Expr::value(call.require::<dyn Clock>()?.now()),
                )
                .filter(principal::Column::Id.eq(id.to_string()));
            if let Some(login) = &params.login {
                update = update.col_expr(principal::Column::Login, Expr::value(login.as_str()));
            }
            if let Some(lang) = &params.lang {
                update = update.col_expr(principal::Column::Lang, Expr::value(lang.as_str()));
            }
            if rights_changed {
                update = update.col_expr(
                    principal::Column::AclGen,
                    Expr::col(principal::Column::AclGen).add(1),
                );
            }
            update.exec(tx).await.map_err(login_taken)?;
            Ok(Saved { id })
        })
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
