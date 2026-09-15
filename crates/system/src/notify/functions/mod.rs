pub mod internal;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::types::NotificationLevel;

use super::api::{Problem, Severity};
use super::entity::{message, problem};
use super::messages::{Note, note_of};
use super::problems::problem_of;

pub const MESSAGE_LIST: &str = "notify.message.list";
pub const MESSAGE_MARK: &str = "notify.message.mark";
pub const MESSAGE_CLEAR: &str = "notify.message.clear";
pub const PROBLEM_LIST: &str = "notify.problem.list";
pub const PROBLEM_GET: &str = "notify.problem.get";
pub const PERMISSION_READ: &str = "notify.read";
pub const PERMISSION_MANAGE: &str = "notify.manage";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Mark {
    #[schemars(length(min = 1))]
    pub ids: Vec<Id>,
    #[serde(default = "read_by_default")]
    pub read: bool,
}

fn read_by_default() -> bool {
    true
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Nothing {}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Marked {
    pub marked: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Deleted {
    pub deleted: u64,
}

struct MessageSource;

#[async_trait]
impl ListSource for MessageSource {
    type Row = Note;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Note>> {
        let own = message::Entity::find()
            .filter(message::Column::PrincipalId.eq(call.principal().id.to_string()));
        let page = fetch_page(own, call, spec).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(note_of)
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

fn level_tones() -> [(&'static str, Tone); 3] {
    [
        (NotificationLevel::Info.as_str(), Tone::Info),
        (NotificationLevel::Warning.as_str(), Tone::Warning),
        (NotificationLevel::Error.as_str(), Tone::Danger),
    ]
}

pub fn message_list() -> FunctionDef {
    let levels: Vec<&str> = NotificationLevel::ALL
        .iter()
        .map(|level| level.as_str())
        .collect();
    function::<ListQuery, Page<Note>>(MESSAGE_LIST)
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .owned::<message::Entity>()
        .ui(List::new("notify.message.list.title")
            .search(["key"])
            .column(Column::new("key", ColumnType::Text).sortable().primary())
            .column(
                Column::new("level", ColumnType::Status)
                    .map(level_tones())
                    .filter_options(levels.iter().copied()),
            )
            .column(Column::new("source", ColumnType::Text))
            .column(
                Column::new("unread", ColumnType::Boolean)
                    .sortable()
                    .filter_options(["true", "false"]),
            )
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .column(Column::new("readAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("mark_read", ActionRole::Group)
                    .function(MESSAGE_MARK)
                    .icon("check"),
            )
            .action(
                Action::new("clear", ActionRole::Button)
                    .function(MESSAGE_CLEAR)
                    .icon("trash-2")
                    .confirm(),
            )
            .row_actions(["mark_read"])
            .empty("notify.message.empty", None)
            .invalidate_on(["notify.message.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(MessageSource)
}

pub fn message_mark() -> FunctionDef {
    function::<Mark, Marked>(MESSAGE_MARK)
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<message::Entity>()
        .key("ids")
        .handler(|call: Call<Mark>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let params = call.params();
            let read_at = params.read.then_some(now);
            let marked = message::Entity::update_many()
                .filter(message::Column::Id.is_in(params.ids.iter().map(ToString::to_string)))
                .filter(message::Column::PrincipalId.eq(call.principal().id.to_string()))
                .col_expr(message::Column::ReadAt, Expr::value(read_at))
                .exec(call.tx()?)
                .await?;
            Ok(Marked {
                marked: marked.rows_affected,
            })
        })
}

pub fn message_clear() -> FunctionDef {
    function::<Nothing, Deleted>(MESSAGE_CLEAR)
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .unowned()
        .handler(|call: Call<Nothing>| async move {
            let deleted = message::Entity::delete_many()
                .filter(message::Column::PrincipalId.eq(call.principal().id.to_string()))
                .filter(message::Column::ReadAt.is_not_null())
                .exec(call.tx()?)
                .await?;
            Ok(Deleted {
                deleted: deleted.rows_affected,
            })
        })
}

struct ProblemSource;

#[async_trait]
impl ListSource for ProblemSource {
    type Row = Problem;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Problem>> {
        let visible = call.account_scope().apply(problem::Entity::find());
        let page = fetch_page(visible, call, spec).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(problem_of)
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

fn severity_tones() -> [(&'static str, Tone); 2] {
    [
        (Severity::Warning.as_str(), Tone::Warning),
        (Severity::Critical.as_str(), Tone::Danger),
    ]
}

pub fn problem_list() -> FunctionDef {
    let severities: Vec<&str> = Severity::ALL
        .iter()
        .map(|severity| severity.as_str())
        .collect();
    function::<ListQuery, Page<Problem>>(PROBLEM_LIST)
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .unowned()
        .key("id")
        .ui(List::new("notify.problem.list.title")
            .search(["key", "subject"])
            .column(Column::new("key", ColumnType::Text).sortable().primary())
            .column(
                Column::new("severity", ColumnType::Status)
                    .sortable()
                    .map(severity_tones())
                    .filter_options(severities.iter().copied()),
            )
            .column(Column::new("subject", ColumnType::Text).sortable())
            .column(Column::new("count", ColumnType::Number).sortable())
            .column(
                Column::new("open", ColumnType::Boolean)
                    .sortable()
                    .filter_options(["true", "false"]),
            )
            .column(Column::new("raisedAt", ColumnType::Datetime).sortable())
            .column(Column::new("clearedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .empty("notify.problem.empty", None)
            .invalidate_on(["notify.problem.*"]))
        .default_sort("raisedAt", SortDir::Desc)
        .source(ProblemSource)
}

pub fn problem_get() -> FunctionDef {
    function::<Pick, Problem>(PROBLEM_GET)
        .kind(Kind::Get)
        .access(Level::User, PERMISSION_READ)
        .unowned()
        .key("id")
        .handler(|call: Call<Pick>| async move {
            let not_found = || KeelError::not_found("keel.error.not_found");
            let row = problem::Entity::find_by_id(call.params().id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(not_found)?;
            let visible = match &row.account_id {
                Some(account) => {
                    let account: Id = account.parse().map_err(|_| not_found())?;
                    call.account_scope().includes(call.db(), account).await?
                }
                None => call.principal().level >= Level::Admin,
            };
            if !visible {
                return Err(not_found());
            }
            problem_of(row)
        })
}
