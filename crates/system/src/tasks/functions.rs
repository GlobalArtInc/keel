use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::system::TaskRunner;
use keel_sdk::types::TaskState;

use super::entity::job;
use super::log;
use super::runner::{ERROR_NOT_FOUND, state_of};

pub const PERMISSION_READ: &str = "task.read";
pub const PERMISSION_MANAGE: &str = "task.manage";
pub const LOG_TAIL: usize = 50;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct TaskRow {
    pub id: String,
    pub function: String,
    pub class: String,
    pub state: TaskState,
    pub progress: String,
    pub done: u64,
    pub total: Option<u64>,
    pub message: Option<String>,
    pub resumable: bool,
    pub request_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub account_id: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub principal_id: String,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct TaskDetail {
    #[serde(flatten)]
    pub row: TaskRow,
    pub params: Value,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub log_tail: Vec<String>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Cancelled {
    pub id: Id,
    pub state: TaskState,
}

pub fn progress_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!("{done}/{total}"),
        None => done.to_string(),
    }
}

fn row_of(row: job::Model) -> Result<TaskRow> {
    let state = state_of(&row)?;
    let done = u64::try_from(row.progress_done).unwrap_or_default();
    let total = row
        .progress_total
        .and_then(|total| u64::try_from(total).ok());
    Ok(TaskRow {
        id: row.id,
        function: row.function,
        class: row.class,
        state,
        progress: progress_text(done, total),
        done,
        total,
        message: row.message,
        resumable: row.resumable,
        request_id: row.request_id,
        created_at: row.created_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        account_id: row.account_id,
        principal_id: row.principal_id,
    })
}

struct TaskSource;

#[async_trait]
impl ListSource for TaskSource {
    type Row = TaskRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<TaskRow>> {
        let page = fetch_page(call.account_scope().apply(job::Entity::find()), call, spec).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(row_of)
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn list() -> FunctionDef {
    let states: Vec<&str> = TaskState::ALL.iter().map(|state| state.as_str()).collect();
    function::<ListQuery, Page<TaskRow>>("task.list")
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .owned::<job::Entity>()
        .ui(List::new("task.list.title")
            .search(["function"])
            .column(
                Column::new("function", ColumnType::Link)
                    .to("task.get", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .sortable()
                    .map([
                        (TaskState::Queued.as_str(), Tone::Neutral),
                        (TaskState::Running.as_str(), Tone::Info),
                        (TaskState::Done.as_str(), Tone::Success),
                        (TaskState::Failed.as_str(), Tone::Danger),
                        (TaskState::Cancelled.as_str(), Tone::Warning),
                        (TaskState::Interrupted.as_str(), Tone::Warning),
                    ])
                    .filter_options(states.iter().copied()),
            )
            .column(Column::new("progress", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("finishedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("open", ActionRole::Button)
                    .screen("task.get")
                    .params([("id", "$id")])
                    .icon("eye")
                    .default(),
            )
            .action(
                Action::new("cancel", ActionRole::Button)
                    .function("task.cancel")
                    .params([("id", "$id")])
                    .icon("x")
                    .confirm()
                    .hidden_if("!(state in ['queued', 'running'])"),
            )
            .row_actions(["open", "cancel"])
            .empty("task.empty", None)
            .invalidate_on(["task.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(TaskSource)
}

pub fn get() -> FunctionDef {
    function::<Pick, TaskDetail>("task.get")
        .kind(Kind::Get)
        .access(Level::User, PERMISSION_READ)
        .owned::<job::Entity>()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id;
            let row = job::Entity::find_by_id(id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found(ERROR_NOT_FOUND).param("id", id.to_string()))?;
            let data_dir = call.require::<super::api::Drain>()?.data_dir()?;
            let log_tail = log::tail(&data_dir, id, LOG_TAIL).await?;
            let params = row.params_masked_json.clone();
            let checkpoint = row.checkpoint_json.clone();
            let result = row.result_json.clone();
            let error = row.error_json.clone();
            Ok(TaskDetail {
                row: row_of(row)?,
                params,
                checkpoint,
                result,
                error,
                log_tail,
            })
        })
}

pub fn cancel() -> FunctionDef {
    function::<Pick, Cancelled>("task.cancel")
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<job::Entity>()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id;
            let runner = call.require::<dyn TaskRunner>()?;
            runner.cancel(id).await?;
            let state = runner.state(id).await?;
            Ok(Cancelled { id, state })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_reads_as_done_over_total_or_done_alone() {
        assert_eq!(progress_text(3, Some(3)), "3/3");
        assert_eq!(progress_text(2, None), "2");
        assert_eq!(progress_text(0, Some(10)), "0/10");
    }
}
