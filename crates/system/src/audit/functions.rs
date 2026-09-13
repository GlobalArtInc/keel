use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;

use super::entity::call;
use super::sink::{STATUS_COMMITTED, STATUS_FAILED};

pub const PERMISSION_READ: &str = "audit.read";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    pub id: Id,
}

pub fn list() -> FunctionDef {
    function::<ListQuery, Page<call::Model>>("audit.list")
        .kind(Kind::List)
        .access(Level::User, PERMISSION_READ)
        .owned::<call::Entity>()
        .ui(List::new("audit.list.title")
            .search(["function", "principalLogin", "requestId"])
            .column(Column::new("at", ColumnType::Datetime).sortable())
            .column(
                Column::new("function", ColumnType::Link)
                    .to("audit.get", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(Column::new("principalLogin", ColumnType::Text).sortable())
            .column(Column::new("principalId", ColumnType::Text))
            .column(Column::new("actorLogin", ColumnType::Text))
            .column(Column::new("via", ColumnType::Text))
            .column(Column::new("ip", ColumnType::Text))
            .column(Column::new("subject", ColumnType::Text))
            .column(
                Column::new("status", ColumnType::Status)
                    .sortable()
                    .map([
                        (STATUS_COMMITTED, Tone::Success),
                        (STATUS_FAILED, Tone::Danger),
                    ])
                    .filter_options([STATUS_COMMITTED, STATUS_FAILED]),
            )
            .column(Column::new("errorKey", ColumnType::Text))
            .column(Column::new("elapsedMs", ColumnType::Number).sortable())
            .column(Column::new("requestId", ColumnType::Text))
            .empty("audit.empty", None))
        .default_sort("at", SortDir::Desc)
        .source(entity_source::<call::Entity>())
}

pub fn get() -> FunctionDef {
    function::<Pick, call::Model>("audit.get")
        .kind(Kind::Get)
        .access(Level::User, PERMISSION_READ)
        .owned::<call::Entity>()
        .handler(|call: Call<Pick>| async move {
            call::Entity::find_by_id(call.params().id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
        })
}
