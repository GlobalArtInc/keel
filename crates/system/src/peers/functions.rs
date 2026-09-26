use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::EntityTrait;
use keel_sdk::serde_json;

use super::entity::product;

pub const LIST: &str = "peer.list";
pub const GET: &str = "peer.get";
pub const STATUS_ACTIVE: &str = "active";
pub const ERROR_STORED: &str = "peer.error.stored";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct Scopes {
    functions: Vec<String>,
    level: Level,
    #[serde(default)]
    on_behalf: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct Peer {
    id: String,
    instance_id: String,
    products: Vec<String>,
    keel: String,
    protocol: i32,
    url: Option<String>,
    public_key: String,
    scopes: Scopes,
    status: String,
    last_seen_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

fn peer_of(row: product::Model) -> Result<Peer> {
    let stored = |error: serde_json::Error| {
        KeelError::internal(ERROR_STORED)
            .param("id", row.id.clone())
            .source(error)
    };
    let products = serde_json::from_value(row.products_json.clone()).map_err(stored)?;
    let scopes = serde_json::from_value(row.inbound_scopes_json.clone()).map_err(stored)?;
    Ok(Peer {
        id: row.id,
        instance_id: row.instance_id,
        products,
        keel: row.keel_version,
        protocol: row.protocol,
        url: row.url,
        public_key: row.public_key,
        scopes,
        status: row.status,
        last_seen_at: row.last_seen_at,
        created_at: row.created_at,
    })
}

struct PeerSource;

#[async_trait]
impl ListSource for PeerSource {
    type Row = Peer;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<Peer>> {
        let page = fetch_page(product::Entity::find(), call, spec).await?;
        Ok(Page {
            data: page.data.into_iter().map(peer_of).collect::<Result<_>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

pub(super) fn list() -> FunctionDef {
    function::<ListQuery, Page<Peer>>(LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .ui(List::new("peer.list.title")
            .search(["url", "instanceId"])
            .column(Column::new("url", ColumnType::Text).sortable().primary())
            .column(Column::new("products", ColumnType::Text))
            .column(Column::new("keel", ColumnType::Text))
            .column(Column::new("protocol", ColumnType::Number).sortable())
            .column(
                Column::new("status", ColumnType::Status)
                    .sortable()
                    .map([(STATUS_ACTIVE, Tone::Success)]),
            )
            .column(Column::new("lastSeenAt", ColumnType::Datetime).sortable())
            .column(Column::new("createdAt", ColumnType::Datetime).sortable())
            .empty("peer.empty", None)
            .invalidate_on(["peer.*"]))
        .default_sort("createdAt", SortDir::Desc)
        .source(PeerSource)
}

pub(super) fn get() -> FunctionDef {
    function::<Pick, Peer>(GET)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .handler(|call: Call<Pick>| async move {
            let row = product::Entity::find_by_id(call.params().id.to_string())
                .one(call.db())
                .await?
                .ok_or_else(|| KeelError::not_found("keel.error.not_found"))?;
            peer_of(row)
        })
}
