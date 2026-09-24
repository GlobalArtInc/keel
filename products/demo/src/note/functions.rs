use keel::prelude::*;
use keel::sdk::chrono::{Days, NaiveDate};
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
};
use keel::sdk::serde_json::{self, Map};
use keel::types::{AccountState, ErrorCode};

use super::{
    Batch, DELETE_BATCH_THRESHOLD, Deleted, GROUP_BATCH_THRESHOLD, NoteEdit, NoteKind, Pick, item,
};

pub const DUE_AHEAD_DAYS: u64 = 7;
const WITHHELD_FEATURE: &str = "demo.withheld";

#[derive(Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Recompute {
    pub values: Map<String, Value>,
    pub changed: String,
}

#[derive(Default, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct RecomputedValues {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<NaiveDate>,
}

#[derive(Default, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Recomputed {
    pub values: RecomputedValues,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
struct Withheld {}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<item::Model>>("note.list")
        .kind(Kind::List)
        .access(Level::User, "note.read")
        .owned::<item::Entity>()
        .ui(List::new("note.list.title")
            .search(["title"])
            .column(
                Column::new("title", ColumnType::Link)
                    .to("note.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("kind", ColumnType::Status)
                    .map([
                        (NoteKind::Memo.as_str(), Tone::Neutral),
                        (NoteKind::Todo.as_str(), Tone::Warning),
                    ])
                    .filter_options([NoteKind::Memo.as_str(), NoteKind::Todo.as_str()]),
            )
            .column(Column::new("tags", ColumnType::Text))
            .column(Column::new("due", ColumnType::Date).sortable())
            .column(Column::new("updatedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen("note.edit")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("note.edit")
                    .default(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("note.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("title".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .action(
                Action::new("export_task", ActionRole::Button)
                    .function("note.export")
                    .params([("format", "csv")])
                    .icon("download"),
            )
            .action(
                Action::new("export", ActionRole::Export)
                    .formats([ExportFormat::Csv, ExportFormat::Json]),
            )
            .row_actions(["edit", "delete"])
            .empty("note.empty", Some("create"))
            .invalidate_on(["note.*"]))
        .default_sort("updatedAt", SortDir::Desc)
        .source(entity_source::<item::Entity>())
}

pub fn get() -> FunctionDef {
    function::<Pick, item::Model>("note.get")
        .kind(Kind::Get)
        .access(Level::User, "note.read")
        .owned::<item::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub fn edit() -> FunctionDef {
    function::<NoteEdit, item::Model>("note.edit")
        .kind(Kind::Form)
        .access(Level::User, "note.manage")
        .owned::<item::Entity>()
        .ui(Form::new("note.edit.title")
            .read("note.get")
            .page("main", ["title", "body", "kind", "due", "tags"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'")
            .field("kind")
            .recompute("note.recompute")
            .field("due")
            .show_if("kind == 'todo'")
            .required_if("kind == 'todo'")
            .multi(["kind", "due", "tags"]))
        .batch_threshold(GROUP_BATCH_THRESHOLD)
        .handler(|call: Call<NoteEdit>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let current = match &params.id {
                Some(id) => Some(load(tx, id).await?),
                None => None,
            };
            let account = match (params.account, &current) {
                (None, Some(row)) => row.account_id.clone(),
                (requested, _) => call.resolve_account(requested).await?.to_string(),
            };
            let owner = call
                .invoke("keel.account.get", json!({ "id": account }))
                .await?;
            let state: AccountState = serde_json::from_value(owner["state"].clone())?;
            if state != AccountState::Active {
                return Err(KeelError::conflict("note.error.account_suspended")
                    .param("state", owner["state"].clone()));
            }
            if params.kind == NoteKind::Todo && params.due.is_none() {
                return Err(KeelError::validation("keel.schema.required")
                    .field("due")
                    .param("property", "due"));
            }
            refuse_taken_title(tx, &account, &params.title, current.as_ref()).await?;
            let now = call.require::<dyn Clock>()?.now();
            let id = match &current {
                Some(row) => {
                    item::ActiveModel {
                        id: Set(row.id.clone()),
                        account_id: Set(account.clone()),
                        title: Set(params.title.clone()),
                        body: Set(params.body.clone()),
                        kind: Set(params.kind.as_str().to_owned()),
                        tags: Set(params.tags.clone()),
                        due: Set(params.due),
                        updated_at: Set(now),
                        ..Default::default()
                    }
                    .update(tx)
                    .await?;
                    row.id.clone()
                }
                None => {
                    let id = Id::generate().to_string();
                    item::ActiveModel {
                        id: Set(id.clone()),
                        account_id: Set(account.clone()),
                        title: Set(params.title.clone()),
                        body: Set(params.body.clone()),
                        kind: Set(params.kind.as_str().to_owned()),
                        tags: Set(params.tags.clone()),
                        due: Set(params.due),
                        created_at: Set(now),
                        updated_at: Set(now),
                    }
                    .insert(tx)
                    .await?;
                    id
                }
            };
            let subject = EventSubject {
                kind: "note".to_owned(),
                id: id
                    .parse()
                    .map_err(|_| KeelError::internal("note.error.id").param("id", id.as_str()))?,
                account_id: Some(account.parse().map_err(|_| {
                    KeelError::internal("note.error.account").param("account", account.as_str())
                })?),
            };
            call.emit_about(
                "note.saved",
                subject,
                &json!({ "id": id, "account_id": account }),
            )?;
            let saved = call.invoke("note.get", json!({ "id": id })).await?;
            Ok(serde_json::from_value(saved)?)
        })
}

pub fn delete() -> FunctionDef {
    function::<Batch, Deleted>("note.delete")
        .kind(Kind::Action)
        .access(Level::User, "note.manage")
        .key("ids")
        .owned::<item::Entity>()
        .batch_threshold(DELETE_BATCH_THRESHOLD)
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let deleted = item::Entity::delete_many()
                .filter(item::Column::Id.is_in(ids))
                .exec(call.tx()?)
                .await?
                .rows_affected;
            for id in &call.params().ids {
                call.emit("note.deleted", &json!({ "id": id }))?;
            }
            Ok(Deleted { deleted })
        })
}

pub fn recompute() -> FunctionDef {
    function::<Recompute, Recomputed>("note.recompute")
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, "note.manage")
        .unowned()
        .handler(|call: Call<Recompute>| async move {
            let params = call.params();
            let kind: Option<NoteKind> = params
                .values
                .get("kind")
                .and_then(|value| serde_json::from_value(value.clone()).ok());
            let due_empty = params
                .values
                .get("due")
                .is_none_or(|due| due.is_null() || due.as_str().is_some_and(str::is_empty));
            if params.changed != "kind" || kind != Some(NoteKind::Todo) || !due_empty {
                return Ok(Recomputed::default());
            }
            let today = call
                .require::<dyn Clock>()?
                .now()
                .with_timezone(&call.tz())
                .date_naive();
            let due = today
                .checked_add_days(Days::new(DUE_AHEAD_DAYS))
                .ok_or_else(|| KeelError::new(ErrorCode::Internal))?;
            Ok(Recomputed {
                values: RecomputedValues { due: Some(due) },
            })
        })
}

pub fn withheld() -> FunctionDef {
    function::<Withheld, Withheld>("note.withheld")
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, "note.read")
        .feature(WITHHELD_FEATURE)
        .unowned()
        .handler(|_: Call<Withheld>| async { Ok(Withheld {}) })
}

async fn refuse_taken_title<C: ConnectionTrait>(
    db: &C,
    account: &str,
    title: &str,
    current: Option<&item::Model>,
) -> Result<()> {
    let mut taken = item::Entity::find()
        .filter(item::Column::AccountId.eq(account))
        .filter(item::Column::Title.eq(title));
    if let Some(row) = current {
        taken = taken.filter(item::Column::Id.ne(row.id.as_str()));
    }
    match taken.one(db).await? {
        Some(_) => Err(KeelError::conflict("note.error.exists")
            .field("title")
            .param("title", title)),
        None => Ok(()),
    }
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<item::Model> {
    item::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
