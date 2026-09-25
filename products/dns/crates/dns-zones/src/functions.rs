use keel::prelude::*;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
};
use keel::types::ErrorCode;

use crate::api::{Expansion, RecordSeed, ZoneTemplates};
use crate::changes::{bump, first_serial};
use crate::provider::DnsProvider;
use crate::{EXISTS, zone};

const READ: &str = "dns.zone.read";
const MANAGE: &str = "dns.zone.manage";
const FEATURE: &str = "dns.zones";
const DNSSEC: &str = "dns.dnssec";
const DNSSEC_GET: &str = "dns.zone.dnssec.get";
const SUBJECT: &str = "dns.zone";
const RECORDS: &str = "dns.record.list";
const RECORD_CREATE: &str = "dns.record.create";
const TEMPLATES: &str = "dns.template.options";
const TEMPLATE_RECORD: &str = "dns.zone.error.template";

const REFRESH: u32 = 10_800;
const RETRY: u32 = 3_600;
const EXPIRE: u32 = 604_800;
const MINIMUM: u32 = 3_600;

fn refresh() -> u32 {
    REFRESH
}

fn retry() -> u32 {
    RETRY
}

fn expire() -> u32 {
    EXPIRE
}

fn minimum() -> u32 {
    MINIMUM
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Pick {
    id: Id,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "snake_case")]
#[schemars(crate = "keel::sdk::schemars", rename_all = "snake_case")]
enum Signing {
    Unsigned,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Dnssec {
    state: Signing,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZoneCreate {
    #[schemars(length(min = 3, max = 253), extend("x-keel" = json!({ "check": "domain" })))]
    name: String,
    #[schemars(extend("x-keel" = json!({ "options": TEMPLATES }), "x-ui" = json!({ "control": "select" })))]
    template: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "check": "ipv4" })))]
    ip: Option<String>,
    #[schemars(extend("x-keel" = json!({ "check": "ipv6" })))]
    ip6: Option<String>,
    #[serde(default = "refresh")]
    #[schemars(range(min = 60, max = 2_419_200))]
    refresh: u32,
    #[serde(default = "retry")]
    #[schemars(range(min = 60, max = 2_419_200))]
    retry: u32,
    #[serde(default = "expire")]
    #[schemars(range(min = 3_600, max = 4_838_400))]
    expire: u32,
    #[serde(default = "minimum")]
    #[schemars(range(min = 60, max = 86_400))]
    minimum: u32,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZoneEdit {
    id: Id,
    #[schemars(range(min = 60, max = 2_419_200))]
    refresh: u32,
    #[schemars(range(min = 60, max = 2_419_200))]
    retry: u32,
    #[schemars(range(min = 3_600, max = 4_838_400))]
    expire: u32,
    #[schemars(range(min = 60, max = 86_400))]
    minimum: u32,
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

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<zone::Model>>("dns.zone.list")
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .ui(List::new("dns.zone.list.title")
            .search(["name"])
            .column(
                Column::new("name", ColumnType::Link)
                    .to("dns.zone.edit", [("id", "$id")])
                    .sortable()
                    .primary(),
            )
            .column(
                Column::new("state", ColumnType::Status)
                    .map([
                        (zone::STATE_PENDING, Tone::Neutral),
                        (zone::STATE_ACTIVE, Tone::Success),
                        (zone::STATE_ERROR, Tone::Danger),
                    ])
                    .filter_options([zone::STATE_PENDING, zone::STATE_ACTIVE, zone::STATE_ERROR]),
            )
            .column(Column::new("serial", ColumnType::Number))
            .column(Column::new("error", ColumnType::Text))
            .column(Column::new("updatedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen("dns.zone.create")
                    .icon("plus"),
            )
            .action(
                Action::new("edit", ActionRole::Edit)
                    .screen("dns.zone.edit")
                    .default(),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function("dns.zone.delete")
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .action(
                Action::new("records", ActionRole::Child)
                    .screen(RECORDS)
                    .params([("zone_id", "$id")])
                    .icon("list"),
            )
            .action(
                Action::new("dnssec", ActionRole::Child)
                    .screen(DNSSEC_GET)
                    .params([("id", "$id")])
                    .icon("shield-check"),
            )
            .row_actions(["edit", "records", "dnssec", "delete"])
            .empty("dns.zone.empty", Some("create"))
            .invalidate_on(["dns.zone.*", "dns.record.*"]))
        .default_sort("name", SortDir::Asc)
        .source(entity_source::<zone::Entity>())
}

pub(crate) fn get() -> FunctionDef {
    function::<Pick, zone::Model>("dns.zone.get")
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .handler(|call: Call<Pick>| async move { load(call.db(), &call.params().id).await })
}

pub(crate) fn dnssec() -> FunctionDef {
    function::<Pick, Dnssec>(DNSSEC_GET)
        .kind(Kind::Get)
        .access(Level::User, READ)
        .feature(DNSSEC)
        .owned::<zone::Entity>()
        .ui(ObjectScreen::new("dns.zone.dnssec.title"))
        .handler(|call: Call<Pick>| async move {
            load(call.db(), &call.params().id).await?;
            Ok(Dnssec {
                state: Signing::Unsigned,
            })
        })
}

pub(crate) fn create() -> FunctionDef {
    function::<ZoneCreate, zone::Model>("dns.zone.create")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .ui(Form::new("dns.zone.create.title")
            .page(
                "main",
                [
                    "name", "template", "ip", "ip6", "refresh", "retry", "expire", "minimum",
                ],
            )
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'")
            .field("ip")
            .show_if("template")
            .field("ip6")
            .show_if("template"))
        .handler(|call: Call<ZoneCreate>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let account = call.resolve_account(params.account).await?;
            let name = params.name.to_ascii_lowercase();
            refuse_taken(tx, &name).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = zone::ActiveModel {
                id: Set(Id::generate().to_string()),
                account_id: Set(account.to_string()),
                name: Set(name),
                kind: Set(zone::KIND_PRIMARY.to_owned()),
                serial: Set(first_serial(now)),
                refresh: Set(seconds(params.refresh)),
                retry: Set(seconds(params.retry)),
                expire: Set(seconds(params.expire)),
                minimum: Set(seconds(params.minimum)),
                template: Set(params.template.as_ref().map(ToString::to_string)),
                state: Set(zone::STATE_PENDING.to_owned()),
                error: Set(None),
                served: Set(false),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            announce(&call, "dns.zone.created", &row)?;
            let row = match &params.template {
                Some(template) => {
                    expand(&call, template, &row).await?;
                    zone::Entity::find_by_id(row.id.clone())
                        .one(tx)
                        .await?
                        .ok_or_else(not_found)?
                }
                None => row,
            };
            apply_after_commit(&call, &row)?;
            Ok(row)
        })
}

async fn expand(call: &Call<ZoneCreate>, template: &Id, row: &zone::Model) -> Result<()> {
    let params = call.params();
    let expansion = Expansion {
        zone: row.name.clone(),
        ip: params.ip.clone(),
        ip6: params.ip6.clone(),
    };
    let seeds = call
        .require::<dyn ZoneTemplates>()?
        .expand(call.tx()?, call.account_scope(), template, &expansion)
        .await?;
    for seed in seeds {
        let mut record = keel::sdk::serde_json::to_value(&seed)?;
        record["zone"] = json!(row.id);
        call.invoke(RECORD_CREATE, record)
            .await
            .map_err(|error| refused_record(error, &seed))?;
    }
    Ok(())
}

fn refused_record(error: KeelError, seed: &RecordSeed) -> KeelError {
    if error.code() != ErrorCode::Validation {
        return error;
    }
    KeelError::validation(TEMPLATE_RECORD)
        .field("template")
        .param("type", seed.kind.as_str())
        .param("name", seed.name.as_str())
}

pub(crate) fn edit() -> FunctionDef {
    function::<ZoneEdit, zone::Model>("dns.zone.edit")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .ui(Form::new("dns.zone.edit.title")
            .read("dns.zone.get")
            .page("main", ["refresh", "retry", "expire", "minimum"]))
        .handler(|call: Call<ZoneEdit>| async move {
            let params = call.params();
            let tx = call.tx()?;
            let now = call.require::<dyn Clock>()?.now();
            let id = params.id.to_string();
            bump(tx, &id, now).await?.ok_or_else(not_found)?;
            let row = zone::ActiveModel {
                id: Set(id),
                refresh: Set(seconds(params.refresh)),
                retry: Set(seconds(params.retry)),
                expire: Set(seconds(params.expire)),
                minimum: Set(seconds(params.minimum)),
                ..Default::default()
            }
            .update(tx)
            .await?;
            announce(&call, "dns.zone.updated", &row)?;
            apply_after_commit(&call, &row)?;
            Ok(row)
        })
}

pub(crate) fn delete() -> FunctionDef {
    function::<Batch, Deleted>("dns.zone.delete")
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<zone::Entity>()
        .handler(|call: Call<Batch>| async move {
            let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
            let tx = call.tx()?;
            let rows = zone::Entity::find()
                .filter(zone::Column::Id.is_in(ids.clone()))
                .all(tx)
                .await?;
            let deleted = zone::Entity::delete_many()
                .filter(zone::Column::Id.is_in(ids))
                .exec(tx)
                .await?
                .rows_affected;
            for row in &rows {
                announce(&call, "dns.zone.deleted", row)?;
            }
            remove_after_commit(&call, rows)?;
            Ok(Deleted { deleted })
        })
}

pub(crate) fn remove_after_commit<P>(call: &Call<P>, rows: Vec<zone::Model>) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let dns = call.require::<dyn DnsProvider>()?;
    call.after_commit(move |ctx| async move { dns.remove(&ctx, &rows).await });
    Ok(())
}

fn apply_after_commit<P>(call: &Call<P>, row: &zone::Model) -> Result<()> {
    let dns = call.require::<dyn DnsProvider>()?;
    let id = row.id.clone();
    call.after_commit(move |ctx| async move { dns.apply(&ctx, &id).await });
    Ok(())
}

fn announce<P>(call: &Call<P>, kind: &str, row: &zone::Model) -> Result<()> {
    let id: Id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal("dns.zone.error.id").param("id", row.id.as_str()))?;
    let account: Id = row.account_id.parse().map_err(|_| {
        KeelError::internal("dns.zone.error.account").param("account", row.account_id.as_str())
    })?;
    call.emit_about(
        kind,
        EventSubject {
            kind: SUBJECT.to_owned(),
            id,
            account_id: Some(account),
        },
        &json!({ "id": row.id, "name": row.name, "serial": row.serial }),
    )
}

fn seconds(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
    let taken = zone::Entity::find()
        .filter(zone::Column::Name.eq(name))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}

async fn load<C: ConnectionTrait>(db: &C, id: &Id) -> Result<zone::Model> {
    zone::Entity::find_by_id(id.to_string())
        .one(db)
        .await?
        .ok_or_else(not_found)
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}
