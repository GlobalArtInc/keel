use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::serde_json;
use keel_sdk::system::Caller;
use keel_sdk::types::PeerScope;

use super::authenticator::stored_principal;
use super::check;
use super::entity::product;
use crate::auth::api::retire_credentials;

pub const LIST: &str = "peer.list";
pub const GET: &str = "peer.get";
pub const DELETE: &str = "peer.delete";
pub const STATUS_ACTIVE: &str = "active";
pub const ERROR_STORED: &str = "peer.error.stored";
pub const EVENT_CREATED: &str = "peer.created";
pub const EVENT_ROTATED: &str = "peer.rotated";
pub const EVENT_DELETED: &str = "peer.deleted";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Pick {
    pub(super) id: Id,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    pub instance_id: String,
    pub products: Vec<String>,
    pub keel: String,
    pub protocol: i32,
    pub url: Option<String>,
    pub public_key: String,
    pub scopes: PeerScope,
    pub status: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct Deleted {
    deleted: bool,
}

fn stored(id: &str) -> impl Fn(serde_json::Error) -> KeelError + '_ {
    move |error| {
        KeelError::internal(ERROR_STORED)
            .param("id", id.to_owned())
            .source(error)
    }
}

pub(super) fn scopes_of(row: &product::Model) -> Result<PeerScope> {
    serde_json::from_value(row.inbound_scopes_json.clone()).map_err(stored(&row.id))
}

pub(super) fn peer_of(row: product::Model) -> Result<Peer> {
    let products = serde_json::from_value(row.products_json.clone()).map_err(stored(&row.id))?;
    let scopes = scopes_of(&row)?;
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

pub(super) async fn retire<P>(call: &Call<P>, row: &product::Model) -> Result<()> {
    let Some(principal) = row.principal_id.as_deref() else {
        return Ok(());
    };
    let principal: Id = principal.parse().map_err(|_| stored_principal(row))?;
    let now = call.require::<dyn Clock>()?.now();
    retire_credentials(call.tx()?, principal, now).await
}

pub(super) async fn by_instance(db: &Db, instance: &str) -> Result<Option<product::Model>> {
    Ok(product::Entity::find()
        .filter(product::Column::InstanceId.eq(instance))
        .one(db)
        .await?)
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
            .action(
                Action::new("create", ActionRole::New)
                    .screen(super::pairing::CREATE)
                    .icon("plus"),
            )
            .action(
                Action::new("pair_code", ActionRole::Button)
                    .screen(super::pairing::PAIR_CODE)
                    .icon("key-round"),
            )
            .action(
                Action::new("rotate", ActionRole::Button)
                    .function(super::rotation::ROTATE)
                    .params([("id", "$id")])
                    .confirm(),
            )
            .action(
                Action::new("delete", ActionRole::Button)
                    .function(DELETE)
                    .params([("id", "$id")])
                    .confirm(),
            )
            .row_actions(["rotate", "delete"])
            .empty("peer.empty", Some("create"))
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
                .ok_or_else(not_found)?;
            peer_of(row)
        })
}

pub(super) fn delete() -> FunctionDef {
    function::<Pick, Deleted>(DELETE)
        .kind(Kind::Action)
        .level(Level::Admin)
        .key("id")
        .unowned()
        .handler(|call: Call<Pick>| async move {
            let id = call.params().id.to_string();
            let row = product::Entity::find_by_id(id.as_str())
                .one(call.tx()?)
                .await?
                .ok_or_else(not_found)?;
            product::Entity::delete_by_id(id.as_str())
                .exec(call.tx()?)
                .await?;
            retire(&call, &row).await?;
            let caller = call.require::<dyn Caller>()?;
            let link = row.id.clone();
            call.after_commit(move |_| async move { check::clear(caller.as_ref(), &link).await });
            call.emit(
                EVENT_DELETED,
                &json!({ "id": row.id, "instanceId": row.instance_id }),
            )?;
            Ok(Deleted { deleted: true })
        })
}

pub(super) fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}
