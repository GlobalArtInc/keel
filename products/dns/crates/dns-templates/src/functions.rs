use keel::prelude::*;
use keel::sdk::escape_like;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};

use crate::expand::visible;
use crate::record::{TemplateRecord, checked};
use crate::template::{self, Records};

const READ: &str = "dns.template.read";
const MANAGE: &str = "dns.template.manage";
const FEATURE: &str = "dns.zones";
const SUBJECT: &str = "dns.template";
const EXISTS: &str = "dns.template.error.exists";
const MAX_RECORDS: usize = 100;
const MAX_OPTIONS: u32 = 100;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TemplateCreate {
    #[schemars(length(min = 1, max = 100))]
    name: String,
    #[serde(default)]
    #[schemars(length(max = MAX_RECORDS), extend("x-ui" = json!({ "control": "list" })))]
    records: Vec<TemplateRecord>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct TemplateEdit {
    id: Id,
    #[schemars(length(min = 1, max = 100))]
    name: String,
    #[schemars(length(max = MAX_RECORDS), extend("x-ui" = json!({ "control": "list" })))]
    records: Vec<TemplateRecord>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Batch {
    #[schemars(length(min = 1))]
    ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Deleted {
    deleted: u64,
}

struct Templates;

#[async_trait]
impl ListSource for Templates {
    type Row = template::Model;

    async fn fetch(
        &self,
        call: &Call<ListQuery>,
        spec: &ListSpec,
    ) -> Result<Page<template::Model>> {
        let select = visible(template::Entity::find(), call.account_scope());
        fetch_page(select, call, spec).await
    }
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<template::Model>>("dns.template.list")
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .ui(List::new("dns.template.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to("dns.template.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("updatedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen("dns.template.create")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("dns.template.edit")
                    .default(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("dns.template.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("dns.template.empty", Some("create"))
            .invalidate_on(["dns.template.*"]))
        .default_sort("name", SortDir::Asc)
        .source(Templates)
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, template::Model>("dns.template.get")
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<Pick>| async move {
            visible(
                template::Entity::find_by_id(call.params().id.to_string()),
                call.account_scope(),
            )
            .one(call.db())
            .await?
            .ok_or_else(not_found)
        })
}

pub(crate) fn create() -> FunctionDef {
    function::<TemplateCreate, template::Model>("dns.template.create")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<template::Entity>()
        .ui(Form::new("dns.template.create.title")
            .page("main", ["name", "records"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
        .handler(|call: Call<TemplateCreate>| async move {
            let params = call.params();
            checked(&params.records)?;
            let tx = call.tx()?;
            let account = call.resolve_account(params.account).await?.to_string();
            let name = params.name.trim().to_owned();
            refuse_taken(tx, &account, &name, None).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = template::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account),
                name: Set(name),
                records: Set(Records(params.records.clone())),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            announce(&call, "dns.template.created", &row)?;
            Ok(row)
        })
}

pub(crate) fn edit() -> FunctionDef {
    function::<TemplateEdit, template::Model>("dns.template.edit")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<template::Entity>()
        .ui(Form::new("dns.template.edit.title")
            .read("dns.template.get")
            .page("main", ["name", "records"]))
        .handler(|call: Call<TemplateEdit>| async move {
            let params = call.params();
            checked(&params.records)?;
            let tx = call.tx()?;
            let id = params.id.to_string();
            let current = template::Entity::find_by_id(id.clone())
                .lock_exclusive()
                .one(tx)
                .await?
                .ok_or_else(not_found)?;
            let name = params.name.trim().to_owned();
            refuse_taken(tx, &current.account_id, &name, Some(&id)).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = template::ActiveModel {
                id: Set(id),
                name: Set(name),
                records: Set(Records(params.records.clone())),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(tx)
            .await?;
            announce(&call, "dns.template.updated", &row)?;
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>("dns.template.delete")
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<template::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let tx = call.tx()?;
            let rows = template::Entity::find()
                .filter(template::Column::Id.is_in(ids.clone()))
                .all(tx)
                .await?;
            let deleted = template::Entity::delete_many()
                .filter(template::Column::Id.is_in(ids))
                .exec(tx)
                .await?
                .rows_affected;
            for row in &rows {
                announce(&call, "dns.template.deleted", row)?;
            }
            Ok(Deleted { deleted })
        })
}

pub(crate) fn options() -> FunctionDef {
    function::<OptionsQuery, Choices>("dns.template.options")
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = query.limit.clamp(1, MAX_OPTIONS);
            let mut select = visible(template::Entity::find(), call.account_scope());
            if !query.search.is_empty() {
                let pattern = format!("%{}%", escape_like(&query.search));
                select = select.filter(Expr::col(template::Column::Name).ilike(pattern.as_str()));
            }
            if let Some(cursor) = &query.cursor {
                select = select.filter(template::Column::Id.gt(cursor.as_str()));
            }
            let mut rows = select
                .order_by_asc(template::Column::Id)
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

fn announce<P>(call: &Call<P>, kind: &str, row: &template::Model) -> Result<()> {
    let id: Id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal("dns.template.error.id").param("id", row.id.as_str()))?;
    let account: Id = row.account_id.parse().map_err(|_| {
        KeelError::internal("dns.template.error.account").param("account", row.account_id.as_str())
    })?;
    call.emit_about(
        kind,
        EventSubject {
            kind: SUBJECT.to_owned(),
            id,
            account_id: Some(account),
        },
        &json!({ "id": row.id, "name": row.name }),
    )
}

async fn refuse_taken<C: ConnectionTrait>(
    db: &C,
    account: &str,
    name: &str,
    except: Option<&str>,
) -> Result<()> {
    let mut select = template::Entity::find()
        .filter(template::Column::AccountId.eq(account))
        .filter(template::Column::Name.eq(name));
    if let Some(id) = except {
        select = select.filter(template::Column::Id.ne(id));
    }
    match select.one(db).await? {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}
