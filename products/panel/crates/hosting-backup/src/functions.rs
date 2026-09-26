use std::path::PathBuf;

use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel::sdk::serde_json;
use keel::sdk::system::{TaskRunner, TaskSpec};
use keel::sdk::tokio;

use crate::config::BackupConfig;
use crate::run::{self, NOT_READY, Restored};
use crate::{
    CLASS, FEATURE, MANAGE, READ, STATE_ACTIVE, STATE_ERROR, STATE_PENDING, backup, names, schedule,
};

pub(crate) const LIST: &str = "hosting.backup.list";
pub(crate) const SITES: &str = "hosting.backup.sites";
pub(crate) const DUE: &str = "hosting.backup.due";

const CREATE: &str = "hosting.backup.create";
const RESTORE: &str = "hosting.backup.restore";
const DELETE: &str = "hosting.backup.delete";
const DOWNLOAD: &str = "hosting.backup.download";
const RUN: &str = "hosting.backup.run";
const SITE_LIST: &str = "hosting.site.list";
const GONE: &str = "hosting.backup.error.gone";
const CONTENT_TYPE: &str = "application/zstd";
const CHOICES: u32 = 20;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct SitePick {
    #[schemars(extend("x-keel" = json!({ "options": SITES })))]
    site: Id,
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

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Started {
    started: u64,
}

#[derive(Deserialize)]
#[serde(crate = "keel::sdk::serde")]
struct SiteChoice {
    id: String,
    domain: String,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<backup::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<backup::Entity>()
        .ui(List::new("hosting.backup.list.title")
            .search(["domain"])
            .column(Column::new("domain", ColumnType::Text).sortable().primary())
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("bytes", ColumnType::Number).sortable())
            .column(Column::new("scheduled", ColumnType::Boolean))
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (STATE_PENDING, Tone::Neutral),
                        (STATE_ACTIVE, Tone::Success),
                        (STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([STATE_PENDING, STATE_ACTIVE, STATE_ERROR]),
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
                Action::new("download", ActionRole::Download)
                    .function(DOWNLOAD)
                    .params([("id", "$id")]),
            )
            .action(
                Action::new("restore", ActionRole::Button)
                    .function(RESTORE)
                    .params([("id", "$id")])
                    .icon("history")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("domain".to_owned()),
                        input: None,
                    }),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("domain".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .action(Action::new("refresh", ActionRole::Refresh))
            .row_actions(["download", "restore", "delete"])
            .empty("hosting.backup.empty", Some("create"))
            .invalidate_on(["hosting.backup.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(entity_source::<backup::Entity>())
}

pub(crate) fn create() -> FunctionDef {
    function::<SitePick, backup::Model>(CREATE)
        .kind(Kind::Form)
        .mode(Mode::Read)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .unowned()
        .class(CLASS)
        .ui(Form::new("hosting.backup.create.title")
            .page("main", ["site"])
            .result(ScreenResult::Task))
        .handler(|call: Call<SitePick>| async move {
            let site = call.params().site;
            run::take(&call, &site, false).await
        })
}

pub(crate) fn restore() -> FunctionDef {
    function::<Pick, Restored>(RESTORE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<backup::Entity>()
        .class(CLASS)
        .ui(ActionScreen::new("hosting.backup.restore.title").result(ScreenResult::Task))
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id;
            run::restore(&call, &id).await
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<backup::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let files = run::drop_backups(call.tx()?, backup::Column::Id.is_in(ids)).await?;
            let deleted = u64::try_from(files.len()).unwrap_or(u64::MAX);
            run::forget_after_commit(&call, files)?;
            Ok(Deleted { deleted })
        })
}

pub(crate) fn download() -> FunctionDef {
    function::<Pick, DownloadFile>(DOWNLOAD)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<backup::Entity>()
        .download(|call: Call<Pick>| async move {
            let id = call.params().id;
            let row = run::load(call.db(), &id).await?;
            if row.state != STATE_ACTIVE {
                return Err(KeelError::conflict(NOT_READY).param("id", id.to_string()));
            }
            let path = PathBuf::from(call.config::<BackupConfig>()?.archive_path(&row.file));
            if tokio::fs::symlink_metadata(&path).await.is_err() {
                return Err(KeelError::not_found(GONE).param("id", id.to_string()));
            }
            Ok(Download::file(
                names::download_of(&row.domain, row.created_at),
                CONTENT_TYPE,
                path,
            )
            .into())
        })
}

pub(crate) fn sites() -> FunctionDef {
    function::<OptionsQuery, Choices>(SITES)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = if query.limit == 0 {
                CHOICES
            } else {
                query.limit.min(CHOICES)
            };
            let page = call
                .invoke(
                    SITE_LIST,
                    json!({ "page": 1, "pageSize": limit, "search": query.search }),
                )
                .await?;
            let rows: Vec<SiteChoice> = serde_json::from_value(page["data"].clone())?;
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|site| Choice::new(site.id, site.domain))
                    .collect(),
                more: false,
            })
        })
}

pub(crate) fn scheduled_run() -> FunctionDef {
    function::<SitePick, backup::Model>(RUN)
        .kind(Kind::Action)
        .mode(Mode::Read)
        .level(Level::Admin)
        .feature(FEATURE)
        .unowned()
        .internal()
        .class(CLASS)
        .handler(|call: Call<SitePick>| async move {
            let site = call.params().site;
            run::take(&call, &site, true).await
        })
}

pub(crate) fn due() -> FunctionDef {
    function::<Nothing, Started>(DUE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .feature(FEATURE)
        .unowned()
        .internal()
        .handler(|call: Call<Nothing>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let due = schedule::Entity::find()
                .filter(schedule::Column::NextAt.lte(now))
                .order_by_asc(schedule::Column::NextAt)
                .all(tx)
                .await?;
            let mut sites = Vec::with_capacity(due.len());
            for row in due {
                let (_, next) = names::next_run(&row.schedule, now)?;
                schedule::Entity::update_many()
                    .col_expr(schedule::Column::NextAt, Expr::value(next))
                    .col_expr(schedule::Column::UpdatedAt, Expr::value(now))
                    .filter(schedule::Column::Id.eq(row.id.as_str()))
                    .exec(tx)
                    .await?;
                sites.push(row.site_id);
            }
            let started = u64::try_from(sites.len()).unwrap_or(u64::MAX);
            spawn_after_commit(&call, sites)?;
            Ok(Started { started })
        })
}

fn spawn_after_commit<P>(call: &Call<P>, sites: Vec<String>) -> Result<()> {
    if sites.is_empty() {
        return Ok(());
    }
    let runner = call.require::<dyn TaskRunner>()?;
    call.after_commit(move |ctx| async move {
        for site in sites {
            runner
                .spawn(TaskSpec {
                    function: RUN.to_owned(),
                    params: json!({ "site": site }),
                    principal: ctx.principal().clone(),
                    class: CLASS.to_owned(),
                    resumable: false,
                    request_id: Some(ctx.request_id().to_owned()),
                })
                .await?;
        }
        Ok(())
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restore_is_a_write_task_and_a_backup_a_read_task() {
        let restore = restore();
        assert_eq!(restore.mode(), Mode::Write);
        assert_eq!(restore.class(), CLASS);
        for def in [create(), scheduled_run()] {
            assert_eq!(def.mode(), Mode::Read, "{}", def.name());
            assert_eq!(def.class(), CLASS, "{}", def.name());
        }
    }
}
