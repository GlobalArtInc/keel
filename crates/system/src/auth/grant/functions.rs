use std::collections::BTreeMap;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::escape_like;
use keel_sdk::list::DEFAULT_MAX_PAGE_SIZE;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel_sdk::sea_orm::sea_query::{Condition, Expr, OnConflict};
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::{PermissionCatalogue, PrincipalSource};

use super::super::entity::grant;
use super::super::functions::{below_caller, below_caller_with_account};
use super::{PERMISSION_MANAGE, PERMISSION_READ};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct GrantSet {
    pub principal: Id,
    #[schemars(length(min = 1, max = 128))]
    pub key: String,
    pub granted: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct GrantSaved {
    pub id: Id,
    pub principal: Id,
    pub key: String,
    pub granted: bool,
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
pub struct GrantDeleted {
    pub deleted: bool,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel_sdk::schemars",
    rename = "Grant",
    rename_all = "camelCase"
)]
pub struct GrantRow {
    pub id: String,
    pub principal_id: String,
    pub principal_login: String,
    pub account_id: String,
    pub key: String,
    pub granted: bool,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

struct GrantSource;

#[async_trait]
impl ListSource for GrantSource {
    type Row = GrantRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<GrantRow>> {
        let mut select = call.account_scope().apply(grant::Entity::find());
        let search = call.params().search.as_str();
        if !search.is_empty() {
            let pattern = format!("%{}%", escape_like(search));
            select = select.filter(
                Condition::any()
                    .add(Expr::col(grant::Column::Key.as_column_ref()).ilike(pattern.as_str()))
                    .add(
                        grant::Column::PrincipalId.is_in(principals_matching(call, search).await?),
                    ),
            );
        }
        let unsearched = ListSpec {
            search: Vec::new(),
            ..spec.clone()
        };
        let page = fetch_page(select, call, &unsearched).await?;
        let logins = logins_of(call, &page.data).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(|row| row_of(row, &logins))
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

async fn principals_matching(call: &Call<ListQuery>, search: &str) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let mut page = 1u32;
    loop {
        let answer = call
            .invoke(
                "keel.principal.list",
                json!({ "search": search, "page": page, "pageSize": DEFAULT_MAX_PAGE_SIZE }),
            )
            .await?;
        let rows = answer["data"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let total = answer["total"]
            .as_u64()
            .and_then(|total| usize::try_from(total).ok())
            .unwrap_or(0);
        ids.extend(
            rows.iter()
                .filter_map(|row| row["id"].as_str().map(str::to_owned)),
        );
        if rows.is_empty() || ids.len() >= total {
            return Ok(ids);
        }
        page += 1;
    }
}

async fn logins_of(
    call: &Call<ListQuery>,
    rows: &[grant::Model],
) -> Result<BTreeMap<String, String>> {
    let principals = call.require::<dyn PrincipalSource>()?;
    let mut logins = BTreeMap::new();
    for row in rows {
        if logins.contains_key(&row.principal_id) {
            continue;
        }
        let principal = principals
            .by_id(principal_of(row)?, call.db())
            .await?
            .ok_or_else(|| unknown_principal(&row.id, &row.principal_id))?;
        logins.insert(row.principal_id.clone(), principal.login);
    }
    Ok(logins)
}

fn row_of(row: grant::Model, logins: &BTreeMap<String, String>) -> Result<GrantRow> {
    let principal_login = logins
        .get(&row.principal_id)
        .cloned()
        .ok_or_else(|| unknown_principal(&row.id, &row.principal_id))?;
    Ok(GrantRow {
        id: row.id,
        principal_id: row.principal_id,
        principal_login,
        account_id: row.account_id,
        key: row.key,
        granted: row.granted,
        created_at: row.created_at,
        created_by: row.created_by,
    })
}

fn unknown_principal(id: &str, principal: &str) -> KeelError {
    KeelError::internal("auth.error.grant_principal")
        .param("id", id)
        .param("principal", principal)
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<GrantRow>>("auth.grant.list")
        .kind(Kind::List)
        .access(Level::Reseller, PERMISSION_READ)
        .owned::<grant::Entity>()
        .ui(List::new("auth.grant.list.title")
            .search(["key", "principalLogin"])
            .column(Column::new("principalLogin", ColumnType::Text).primary())
            .column(Column::new("key", ColumnType::Text).sortable())
            .column(Column::new("granted", ColumnType::Boolean).filter_options(["true", "false"]))
            .column(
                Column::new("accountId", ColumnType::Text).filter_remote("keel.account.options"),
            )
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("set", ActionRole::New)
                    .screen("auth.grant.set")
                    .icon("plus"),
            )
            .action(
                Action::new("delete", ActionRole::Button)
                    .function("auth.grant.delete")
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["delete"])
            .empty("auth.grant.empty", Some("set"))
            .invalidate_on(["auth.grant.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(GrantSource)
}

pub fn set() -> FunctionDef {
    function::<GrantSet, GrantSaved>("auth.grant.set")
        .kind(Kind::Form)
        .access(Level::Reseller, PERMISSION_MANAGE)
        .unowned()
        .ui(Form::new("auth.grant.set.title").page("main", ["principal", "key", "granted"]))
        .handler(|call: Call<GrantSet>| async move {
            let params = call.params();
            let catalogue = call.require::<dyn PermissionCatalogue>()?;
            if !catalogue.registered(&params.key) {
                return Err(KeelError::validation("auth.error.unknown_permission")
                    .field("key")
                    .param("key", params.key.as_str()));
            }
            if !catalogue.holds(call.principal(), &params.key) {
                return Err(KeelError::forbidden("auth.error.grant_not_held")
                    .param("key", params.key.as_str()));
            }
            let (target, account) = below_caller_with_account(&call, params.principal).await?;
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let row = grant::Entity::insert(grant::ActiveModel {
                id: Set(Id::generate().to_string()),
                principal_id: Set(target.to_string()),
                account_id: Set(account.to_string()),
                key: Set(params.key.clone()),
                granted: Set(params.granted),
                created_at: Set(now),
                created_by: Set(call.principal().id.to_string()),
            })
            .on_conflict(
                OnConflict::columns([grant::Column::PrincipalId, grant::Column::Key])
                    .update_columns([
                        grant::Column::AccountId,
                        grant::Column::Granted,
                        grant::Column::CreatedAt,
                        grant::Column::CreatedBy,
                    ])
                    .to_owned(),
            )
            .exec_with_returning(tx)
            .await?;
            call.require::<dyn PrincipalSource>()?
                .touch_acl(target, tx)
                .await?;
            Ok(GrantSaved {
                id: id_of(&row)?,
                principal: target,
                key: row.key,
                granted: row.granted,
            })
        })
}

pub fn delete() -> FunctionDef {
    function::<Pick, GrantDeleted>("auth.grant.delete")
        .kind(Kind::Action)
        .access(Level::Reseller, PERMISSION_MANAGE)
        .owned::<grant::Entity>()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id.to_string();
            let tx = call.tx()?;
            let row = grant::Entity::find_by_id(&id)
                .one(tx)
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?;
            let target = below_caller(&call, principal_of(&row)?).await?;
            grant::Entity::delete_by_id(&id).exec(tx).await?;
            call.require::<dyn PrincipalSource>()?
                .touch_acl(target, tx)
                .await?;
            Ok(GrantDeleted { deleted: true })
        })
}

fn id_of(row: &grant::Model) -> Result<Id> {
    row.id
        .parse()
        .map_err(|_| KeelError::internal("auth.error.grant_id").param("id", row.id.as_str()))
}

fn principal_of(row: &grant::Model) -> Result<Id> {
    row.principal_id
        .parse()
        .map_err(|_| unknown_principal(&row.id, &row.principal_id))
}
