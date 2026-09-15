use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;

use super::super::consumer::{STATE_FAILED, STATE_PENDING, STATE_SENT};
use super::super::entity::delivery;

pub const DELIVERY_LIST: &str = "notify.delivery.list";
pub const STATES: [&str; 3] = [STATE_PENDING, STATE_SENT, STATE_FAILED];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub channel_id: String,
    pub channel: String,
    pub message_id: String,
    pub message_key: String,
    pub attempt: i32,
    pub state: String,
    pub next_at: DateTime<Utc>,
    pub error: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

fn delivery_of(row: delivery::Model) -> Delivery {
    Delivery {
        id: row.id,
        channel_id: row.channel_id,
        channel: row.channel,
        message_id: row.message_id,
        message_key: row.message_key,
        attempt: row.attempt,
        state: row.state,
        next_at: row.next_at,
        error: row.error,
        sent_at: row.sent_at,
        created_at: row.created_at,
    }
}

struct DeliverySource;

#[async_trait]
impl ListSource for DeliverySource {
    type Row = Delivery;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Delivery>> {
        let page = fetch_page(delivery::Entity::find(), call, spec).await?;
        Ok(Page {
            data: page.data.into_iter().map(delivery_of).collect(),
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn delivery_list() -> FunctionDef {
    function::<ListQuery, Page<Delivery>>(DELIVERY_LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(List::new("notify.delivery.list.title")
            .search(["channel", "messageKey"])
            .column(
                Column::new("channel", ColumnType::Text)
                    .sortable()
                    .primary(),
            )
            .column(Column::new("messageKey", ColumnType::Text).sortable())
            .column(
                Column::new("state", ColumnType::Status)
                    .sortable()
                    .map([
                        (STATE_PENDING, Tone::Info),
                        (STATE_SENT, Tone::Success),
                        (STATE_FAILED, Tone::Danger),
                    ])
                    .filter_options(STATES),
            )
            .column(Column::new("attempt", ColumnType::Number).sortable())
            .column(Column::new("nextAt", ColumnType::Datetime).sortable())
            .column(Column::new("error", ColumnType::Text))
            .column(Column::new("sentAt", ColumnType::Datetime).sortable())
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .empty("notify.delivery.empty", None)
            .invalidate_on(["notify.channel.*", "notify.delivery.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(DeliverySource)
}
