use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::types::{Scope, is_mask};

use super::super::entity::token;
use super::{Issue, issue, revoke};

pub const PERMISSION_READ: &str = "auth.token.read";
pub const PERMISSION_MANAGE: &str = "auth.token.manage";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct TokenCreate {
    #[schemars(length(min = 1, max = 64))]
    pub name: String,
    pub level: Option<Level>,
    pub scope: Option<Scope>,
    #[schemars(extend("x-keel" = json!({ "check": "cidr" })))]
    pub ip_allow: Option<Vec<String>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct TokenCreated {
    pub id: Id,
    #[schemars(extend("x-keel" = json!({ "once": true })))]
    pub token: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct TokenRevoked {
    pub revoked: bool,
}

struct TokenSource;

#[async_trait]
impl ListSource for TokenSource {
    type Row = token::Model;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<token::Model>> {
        let mut select = call.account_scope().apply(token::Entity::find());
        if call.principal().level < Level::Reseller {
            select = select.filter(token::Column::PrincipalId.eq(call.principal().id.to_string()));
        }
        fetch_page(select, call, spec).await
    }
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<token::Model>>("auth.token.list")
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .owned::<token::Entity>()
        .ui(List::new("auth.token.list.title")
            .search(["name"])
            .column(Column::new("name", ColumnType::Text).sortable().primary())
            .column(
                Column::new("level", ColumnType::Text)
                    .sortable()
                    .filter_options(["service", "user", "reseller", "admin"]),
            )
            .column(Column::new("scope", ColumnType::Text))
            .column(Column::new("ipAllow", ColumnType::Text))
            .column(Column::new("expiresAt", ColumnType::Datetime).sortable())
            .column(Column::new("lastUsedAt", ColumnType::Datetime).sortable())
            .column(Column::new("revokedAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen("auth.token.create")
                    .icon("plus"),
            )
            .action(
                Action::new("revoke", ActionRole::Button)
                    .function("auth.token.revoke")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["revoke"])
            .empty("auth.token.empty", Some("create"))
            .invalidate_on(["auth.token.*"]))
        .default_sort("name", SortDir::Asc)
        .source(TokenSource)
}

pub fn create() -> FunctionDef {
    function::<TokenCreate, TokenCreated>("auth.token.create")
        .kind(Kind::Form)
        .access(Level::User, PERMISSION_MANAGE)
        .unowned()
        .ui(Form::new("auth.token.create.title")
            .page("main", ["name", "level", "scope", "ipAllow", "expiresAt"]))
        .handler(|call: Call<TokenCreate>| async move {
            let params = call.params();
            let caller = call.principal();
            if caller.level >= Level::Owner {
                return Err(KeelError::forbidden("auth.error.token_owner"));
            }
            let level = params.level.unwrap_or(caller.level);
            if level < Level::Service || level > caller.level {
                return Err(KeelError::validation("auth.error.token_level")
                    .field("level")
                    .param("level", level.as_str())
                    .param("caller", caller.level.as_str()));
            }
            let scope = params.scope.clone().unwrap_or_else(|| caller.scope.clone());
            if let Scope::Functions(masks) = &scope {
                for (index, mask) in masks.iter().enumerate() {
                    if !is_mask(mask) {
                        return Err(KeelError::validation("auth.error.token_mask")
                            .field(format!("scope.functions[{index}]"))
                            .param("mask", mask.as_str()));
                    }
                }
            }
            if !caller.scope.contains(&scope) {
                return Err(KeelError::forbidden("auth.error.token_scope"));
            }
            let now = call.require::<dyn Clock>()?.now();
            if params.expires_at.is_some_and(|at| at <= now) {
                return Err(
                    KeelError::validation("auth.error.token_expires_past").field("expiresAt")
                );
            }
            let (id, token) = issue(
                call.tx()?,
                Issue {
                    principal: caller,
                    name: &params.name,
                    level,
                    scope,
                    ip_allow: params.ip_allow.clone().unwrap_or_default(),
                    expires_at: params.expires_at,
                },
                now,
            )
            .await?;
            Ok(TokenCreated {
                id,
                token: token.into_inner(),
            })
        })
}

pub fn revoke_function() -> FunctionDef {
    function::<Pick, TokenRevoked>("auth.token.revoke")
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<token::Entity>()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id.to_string();
            let tx = call.tx()?;
            let row = token::Entity::find_by_id(&id)
                .one(tx)
                .await?
                .ok_or_else(not_found)?;
            let caller = call.principal();
            if caller.level < Level::Reseller && row.principal_id != caller.id.to_string() {
                return Err(not_found());
            }
            let now = call.require::<dyn Clock>()?.now();
            let revoked = revoke(tx, &id, now).await?;
            Ok(TokenRevoked { revoked })
        })
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}
