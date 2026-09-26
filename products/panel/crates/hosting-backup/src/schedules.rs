use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, Set,
};

use crate::functions::SITES;
use crate::names::{self, SCHEDULE_MAX};
use crate::{FEATURE, KEEP_DEFAULT, KEEP_MAX, MANAGE, READ, run, schedule};

const LIST: &str = "hosting.backup.schedule.list";
const GET: &str = "hosting.backup.schedule.get";
const CREATE: &str = "hosting.backup.schedule.create";
const EDIT: &str = "hosting.backup.schedule.edit";
const DELETE: &str = "hosting.backup.schedule.delete";
const EXISTS: &str = "hosting.backup.schedule.error.exists";
const MISSING: &str = "hosting.backup.schedule.error.id";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ScheduleCreate {
    #[schemars(extend("x-keel" = json!({ "options": SITES })))]
    site: Id,
    #[schemars(length(min = 1, max = SCHEDULE_MAX))]
    schedule: String,
    #[serde(default = "keep_default")]
    #[schemars(range(min = 1, max = KEEP_MAX))]
    keep: u32,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ScheduleEdit {
    id: Id,
    #[schemars(length(min = 1, max = SCHEDULE_MAX))]
    schedule: String,
    #[serde(default = "keep_default")]
    #[schemars(range(min = 1, max = KEEP_MAX))]
    keep: u32,
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

fn keep_default() -> u32 {
    u32::try_from(KEEP_DEFAULT).unwrap_or(1)
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<schedule::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<schedule::Entity>()
        .ui(List::new("hosting.backup.schedule.list.title")
            .search(["domain"])
            .column(
                Column::new("domain", ColumnType::Link)
                    .to(EDIT, [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("schedule", ColumnType::Text))
            .column(Column::new("keep", ColumnType::Number))
            .column(Column::new("nextAt", ColumnType::Datetime).sortable())
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
                        items: Some("domain".to_owned()),
                        input: None,
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("hosting.backup.schedule.empty", Some("create"))
            .invalidate_on(["hosting.backup.*"]))
        .default_sort("domain", SortDir::Asc)
        .source(entity_source::<schedule::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, schedule::Model>(GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<schedule::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create() -> FunctionDef {
    function::<ScheduleCreate, schedule::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<schedule::Entity>()
        .ui(Form::new("hosting.backup.schedule.create.title")
            .page("main", ["site", "schedule", "keep"]))
        .handler(|call: Call<ScheduleCreate>| async move {
            let params = call.params();
            let now = call.require::<dyn Clock>()?.now();
            let (text, next_at) = names::next_run(&params.schedule, now)?;
            let site = run::site_of(&call, &params.site).await?;
            let owner: Id = site
                .account_id
                .parse()
                .map_err(|_| KeelError::internal("hosting.backup.error.site_row"))?;
            let owner = call.resolve_account(Some(owner)).await?.to_string();
            let tx = call.tx()?;
            let taken = schedule::Entity::find()
                .filter(schedule::Column::SiteId.eq(site.id.as_str()))
                .count(tx)
                .await?;
            if taken > 0 {
                return Err(KeelError::conflict(EXISTS)
                    .field("site")
                    .param("site", site.domain.as_str()));
            }
            Ok(schedule::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(owner),
                site_id: Set(site.id),
                domain: Set(site.domain),
                schedule: Set(text),
                keep: Set(i32::try_from(params.keep).unwrap_or(i32::MAX)),
                next_at: Set(next_at),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?)
        })
}

pub(crate) fn edit() -> FunctionDef {
    function::<ScheduleEdit, schedule::Model>(EDIT)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<schedule::Entity>()
        .ui(Form::new("hosting.backup.schedule.edit.title")
            .read(GET)
            .page("main", ["schedule", "keep"]))
        .handler(|call: Call<ScheduleEdit>| async move {
            let params = call.params();
            let now = call.require::<dyn Clock>()?.now();
            let (text, next_at) = names::next_run(&params.schedule, now)?;
            let tx = call.tx()?;
            let mut row = load(tx, &params.id).await?.into_active_model();
            row.schedule = Set(text);
            row.keep = Set(i32::try_from(params.keep).unwrap_or(i32::MAX));
            row.next_at = Set(next_at);
            row.updated_at = Set(now);
            Ok(row.update(tx).await?)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<schedule::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let gone = schedule::Entity::delete_many()
                .filter(schedule::Column::Id.is_in(ids))
                .exec(call.tx()?)
                .await?;
            Ok(Deleted {
                deleted: gone.rows_affected,
            })
        })
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<schedule::Model> {
    schedule::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found(MISSING).param("id", id.to_string()))
}
