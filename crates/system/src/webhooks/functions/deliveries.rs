use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::super::consumer::{STATE_DEAD, STATE_DELIVERED, STATE_FAILED, STATE_PENDING, STATES};
use super::super::entity::delivery;
use super::{PERMISSION_MANAGE, Pick};

pub const DELIVERY_LIST: &str = "webhook.delivery.list";
pub const DELIVERY_GET: &str = "webhook.delivery.get";
pub const DELIVERY_RETRY: &str = "webhook.delivery.retry";
pub const SCOPE_SUBSCRIPTION: &str = "subscription_id";
pub const ERROR_NOT_RETRYABLE: &str = "webhook.error.not_retryable";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub subscription_id: String,
    pub event_id: String,
    pub seq: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub state: String,
    pub attempt: i32,
    pub next_at: DateTime<Utc>,
    pub last_status: Option<i32>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller" })))]
    pub account_id: String,
}

pub fn delivery_of(row: delivery::Model) -> Delivery {
    Delivery {
        id: row.id,
        subscription_id: row.subscription_id,
        event_id: row.event_id,
        seq: row.seq,
        kind: row.kind,
        state: row.state,
        attempt: row.attempt,
        next_at: row.next_at,
        last_status: row.last_status,
        last_error: row.last_error,
        delivered_at: row.delivered_at,
        created_at: row.created_at,
        account_id: row.account_id,
    }
}

async fn load(db: &Db, id: Id) -> Result<delivery::Model> {
    delivery::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}

struct DeliverySource;

#[async_trait]
impl ListSource for DeliverySource {
    type Row = Delivery;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Delivery>> {
        let mut select = call.account_scope().apply(delivery::Entity::find());
        if let Some(subscription) = call
            .params()
            .scope
            .get(SCOPE_SUBSCRIPTION)
            .and_then(Value::as_str)
        {
            select = select.filter(delivery::Column::SubscriptionId.eq(subscription));
        }
        let page = fetch_page(select, call, spec).await?;
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
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<delivery::Entity>()
        .ui(List::new("webhook.delivery.list.title")
            .search(["eventId", "type"])
            .column(
                Column::new("eventId", ColumnType::Link)
                    .to(DELIVERY_GET, [("id", "$id")])
                    .primary(),
            )
            .column(Column::new("type", ColumnType::Text).sortable())
            .column(
                Column::new("state", ColumnType::Status)
                    .sortable()
                    .map([
                        (STATE_PENDING, Tone::Neutral),
                        (STATE_DELIVERED, Tone::Success),
                        (STATE_FAILED, Tone::Warning),
                        (STATE_DEAD, Tone::Danger),
                    ])
                    .filter_options(STATES),
            )
            .column(Column::new("attempt", ColumnType::Number).sortable())
            .column(Column::new("nextAt", ColumnType::Datetime).sortable())
            .column(Column::new("lastStatus", ColumnType::Number))
            .column(Column::new("lastError", ColumnType::Text))
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("retry", ActionRole::Button)
                    .function(DELIVERY_RETRY)
                    .params([("id", "$id")])
                    .icon("rotate-cw")
                    .hidden_if(&format!(
                        "state in ['{STATE_PENDING}', '{STATE_DELIVERED}']"
                    )),
            )
            .row_actions(["retry"])
            .empty("webhook.delivery.empty", None)
            .invalidate_on(["webhook.delivery.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(DeliverySource)
}

pub fn delivery_get() -> FunctionDef {
    function::<Pick, Delivery>(DELIVERY_GET)
        .kind(Kind::Get)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<delivery::Entity>()
        .handler(|call: Call<Pick>| async move {
            Ok(delivery_of(load(call.db(), call.params().id).await?))
        })
}

pub fn delivery_retry() -> FunctionDef {
    function::<Pick, Delivery>(DELIVERY_RETRY)
        .kind(Kind::Action)
        .access(Level::User, PERMISSION_MANAGE)
        .owned::<delivery::Entity>()
        .handler(|call: Call<Pick>| async move {
            let now = call.require::<dyn Clock>()?.now();
            let tx = call.tx()?;
            let row = load(tx, call.params().id).await?;
            if row.state != STATE_FAILED && row.state != STATE_DEAD {
                return Err(KeelError::conflict(ERROR_NOT_RETRYABLE)
                    .param("id", row.id.as_str())
                    .param("state", row.state.as_str()));
            }
            delivery::Entity::update_many()
                .filter(delivery::Column::Id.eq(row.id.clone()))
                .col_expr(delivery::Column::State, Expr::value(STATE_PENDING))
                .col_expr(delivery::Column::NextAt, Expr::value(now))
                .exec(tx)
                .await?;
            let delivery = delivery_of(load(tx, call.params().id).await?);
            call.emit_about(
                "webhook.delivery.retried",
                EventSubject {
                    kind: "webhook.delivery".to_owned(),
                    id: call.params().id,
                    account_id: delivery.account_id.parse().ok(),
                },
                &json!({ "id": delivery.id, "subscription": delivery.subscription_id }),
            )?;
            Ok(delivery)
        })
}
