use std::collections::BTreeSet;

use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
};
use keel_dns_zones::api::{ZoneRef, Zones};

use crate::value::{Entry, RecordType};
use crate::{LIMIT, record};

const READ: &str = "dns.zone.read";
const MANAGE: &str = "dns.zone.manage";
const FEATURE: &str = "dns.zones";
const SUBJECT: &str = "dns.record";
const PARENT: &str = "zone_id";
const ZONES: &str = "dns.zone.options";
const ZONE_PARAM: &str = "zone";
const ZONE_CONTEXT: &str = "$screen.params.zone";
const ZONE_FROM_LIST: &str = "$screen.params.zone_id";
const DEFAULT_TTL: u32 = 3_600;
const OWNER: &str = r"^(@|\*|(\*\.)?[A-Za-z0-9_]([A-Za-z0-9_-]{0,61}[A-Za-z0-9_])?(\.[A-Za-z0-9_]([A-Za-z0-9_-]{0,61}[A-Za-z0-9_])?)*)$";

fn ttl() -> u32 {
    DEFAULT_TTL
}

fn value_check() -> Value {
    let cases: keel::sdk::serde_json::Map<String, Value> = RecordType::ALL
        .iter()
        .map(|kind| (kind.as_str().to_owned(), json!(kind.check().as_str())))
        .collect();
    json!({ "checkBy": { "field": "type", "cases": cases } })
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct RecordCreate {
    #[schemars(extend("x-keel" = json!({ "options": ZONES }), "x-ui" = json!({ "control": "select" })))]
    zone: Id,
    #[serde(rename = "type")]
    kind: RecordType,
    #[schemars(length(min = 1, max = 253), pattern(OWNER))]
    name: String,
    #[schemars(
        length(min = 1, max = 4000),
        extend("x-keel" = value_check())
    )]
    value: String,
    #[serde(default = "ttl")]
    #[schemars(range(min = 60, max = 604_800))]
    ttl: u32,
    priority: Option<u16>,
    weight: Option<u16>,
    port: Option<u16>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct RecordEdit {
    id: Id,
    #[serde(rename = "type")]
    kind: RecordType,
    #[schemars(length(min = 1, max = 253), pattern(OWNER))]
    name: String,
    #[schemars(
        length(min = 1, max = 4000),
        extend("x-keel" = value_check())
    )]
    value: String,
    #[schemars(range(min = 60, max = 604_800))]
    ttl: u32,
    priority: Option<u16>,
    weight: Option<u16>,
    port: Option<u16>,
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

struct Records;

#[async_trait]
impl ListSource for Records {
    type Row = record::Model;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<record::Model>> {
        let mut select = call.account_scope().apply(record::Entity::find());
        if let Some(zone) = call.params().scope.get(PARENT) {
            let zone = zone.as_str().ok_or_else(|| {
                KeelError::validation("dns.record.error.parent").field(format!("scope.{PARENT}"))
            })?;
            select = select.filter(record::Column::ZoneId.eq(zone));
        }
        fetch_page(select, call, spec).await
    }
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<record::Model>>("dns.record.list")
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<record::Entity>()
        .ui(List::new("dns.record.list.title")
            .search(["name", "value"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to("dns.record.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("type", ColumnType::Text)
                    .sortable()
                    .filter_options(RecordType::ALL.map(RecordType::as_str)),
            )
            .column(Column::new("value", ColumnType::Text))
            .column(Column::new("ttl", ColumnType::Number).sortable())
            .column(Column::new("priority", ColumnType::Number))
            .column(Column::new("updatedAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("create", ActionRole::New)
                    .screen("dns.record.create")
                    .params([(ZONE_PARAM, ZONE_FROM_LIST)])
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("dns.record.edit")
                    .default(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("dns.record.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["edit", "delete"])
            .empty("dns.record.empty", Some("create"))
            .invalidate_on(["dns.record.*"]))
        .default_sort("name", SortDir::Asc)
        .source(Records)
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, record::Model>("dns.record.get")
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<record::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn create() -> FunctionDef {
    function::<RecordCreate, record::Model>("dns.record.create")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<record::Entity>()
        .ui(conditions(
            Form::new("dns.record.create.title")
                .page(
                    "main",
                    [
                        "zone", "name", "type", "priority", "weight", "port", "value", "ttl",
                    ],
                )
                .field("zone")
                .readonly_if(ZONE_CONTEXT),
        ))
        .handler(|call: Call<RecordCreate>| async move {
            let params = call.params();
            let entry = Entry::checked(
                params.kind,
                &params.name,
                &params.value,
                params.priority,
                params.weight,
                params.port,
            )?;
            let tx = call.tx()?;
            let now = call.require::<dyn Clock>()?.now();
            let zones = call.require::<dyn Zones>()?;
            let zone = zones
                .change(tx, call.account_scope(), &params.zone, now)
                .await?;
            call.resolve_account(Some(zone.account)).await?;
            let kept = record::Entity::find()
                .filter(record::Column::ZoneId.eq(zone.id.to_string()))
                .count(tx)
                .await?;
            call.check_limit(LIMIT, kept).await?;
            let row = record::ActiveModel {
                id: Set(Id::generate().to_string()),
                zone_id: Set(zone.id.to_string()),
                account_id: Set(zone.account.to_string()),
                kind: Set(entry.kind.as_str().to_owned()),
                name: Set(entry.name),
                value: Set(entry.value),
                ttl: Set(seconds(params.ttl)),
                priority: Set(entry.priority),
                weight: Set(entry.weight),
                port: Set(entry.port),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            announce(&call, "dns.record.created", &row, &zone)?;
            publish_after_commit(&call, zone.id)?;
            Ok(row)
        })
}

pub(crate) fn edit() -> FunctionDef {
    function::<RecordEdit, record::Model>("dns.record.edit")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<record::Entity>()
        .ui(conditions(
            Form::new("dns.record.edit.title")
                .read("dns.record.get")
                .page(
                    "main",
                    ["name", "type", "priority", "weight", "port", "value", "ttl"],
                ),
        )
        .multi(["ttl"]))
        .handler(|call: Call<RecordEdit>| async move {
            let params = call.params();
            let entry = Entry::checked(
                params.kind,
                &params.name,
                &params.value,
                params.priority,
                params.weight,
                params.port,
            )?;
            let tx = call.tx()?;
            let now = call.require::<dyn Clock>()?.now();
            let current = load(tx, &params.id).await?;
            let zone = change(&call, &current.zone_id, now).await?;
            let row = record::ActiveModel {
                id: Set(current.id),
                kind: Set(entry.kind.as_str().to_owned()),
                name: Set(entry.name),
                value: Set(entry.value),
                ttl: Set(seconds(params.ttl)),
                priority: Set(entry.priority),
                weight: Set(entry.weight),
                port: Set(entry.port),
                updated_at: Set(now),
                ..Default::default()
            }
            .update(tx)
            .await?;
            announce(&call, "dns.record.updated", &row, &zone)?;
            publish_after_commit(&call, zone.id)?;
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>("dns.record.delete")
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<record::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let tx = call.tx()?;
            let now = call.require::<dyn Clock>()?.now();
            let rows = record::Entity::find()
                .filter(record::Column::Id.is_in(ids.clone()))
                .all(tx)
                .await?;
            let touched: BTreeSet<&str> = rows.iter().map(|row| row.zone_id.as_str()).collect();
            let mut zones = Vec::new();
            for zone in touched {
                zones.push(change(&call, zone, now).await?);
            }
            let deleted = record::Entity::delete_many()
                .filter(record::Column::Id.is_in(ids))
                .exec(tx)
                .await?
                .rows_affected;
            for row in &rows {
                if let Some(zone) = zones.iter().find(|zone| zone.id.to_string() == row.zone_id) {
                    announce(&call, "dns.record.deleted", row, zone)?;
                }
            }
            for zone in zones {
                publish_after_commit(&call, zone.id)?;
            }
            Ok(Deleted { deleted })
        })
}

fn conditions(form: Form) -> Form {
    form.field("priority")
        .show_if("type in ['MX', 'SRV']")
        .required_if("type in ['MX', 'SRV']")
        .field("weight")
        .show_if("type == 'SRV'")
        .required_if("type == 'SRV'")
        .field("port")
        .show_if("type == 'SRV'")
        .required_if("type == 'SRV'")
}

async fn change<P>(
    call: &Call<P>,
    zone: &str,
    now: keel::sdk::chrono::DateTime<keel::sdk::chrono::Utc>,
) -> Result<ZoneRef> {
    let zone: Id = zone
        .parse()
        .map_err(|_| KeelError::internal("dns.record.error.zone").param("zone", zone))?;
    call.require::<dyn Zones>()?
        .change(call.tx()?, call.account_scope(), &zone, now)
        .await
}

fn publish_after_commit<P>(call: &Call<P>, zone: Id) -> Result<()> {
    let zones = call.require::<dyn Zones>()?;
    call.after_commit(move |ctx| async move { zones.publish(&ctx, &zone).await });
    Ok(())
}

fn announce<P>(call: &Call<P>, kind: &str, row: &record::Model, zone: &ZoneRef) -> Result<()> {
    let id: Id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal("dns.record.error.id").param("id", row.id.as_str()))?;
    call.emit_about(
        kind,
        EventSubject {
            kind: SUBJECT.to_owned(),
            id,
            account_id: Some(zone.account),
        },
        &json!({
            "id": row.id,
            "zone": row.zone_id,
            "name": row.name,
            "type": row.kind,
            "serial": zone.serial,
        }),
    )
}

fn seconds(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<record::Model> {
    record::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}
