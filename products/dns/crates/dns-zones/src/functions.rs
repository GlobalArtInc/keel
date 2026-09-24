use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Datelike, Utc};
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set,
};

use crate::provider::DnsProvider;
use crate::{EXISTS, zone};

const READ: &str = "dns.zone.read";
const MANAGE: &str = "dns.zone.manage";
const FEATURE: &str = "dns.zones";
const SUBJECT: &str = "dns.zone";
const SERIAL_REVISIONS: i64 = 100;

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

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct ZoneCreate {
    #[schemars(length(min = 3, max = 253), extend("x-keel" = json!({ "check": "domain" })))]
    name: String,
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
            .row_actions(["edit", "delete"])
            .empty("dns.zone.empty", Some("create"))
            .invalidate_on(["dns.zone.*"]))
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

pub(crate) fn create() -> FunctionDef {
    function::<ZoneCreate, zone::Model>("dns.zone.create")
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .ui(Form::new("dns.zone.create.title")
            .page("main", ["name", "refresh", "retry", "expire", "minimum"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
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
                serial: Set(serial(now, None)),
                refresh: Set(seconds(params.refresh)),
                retry: Set(seconds(params.retry)),
                expire: Set(seconds(params.expire)),
                minimum: Set(seconds(params.minimum)),
                template: Set(None),
                state: Set(zone::STATE_PENDING.to_owned()),
                error: Set(None),
                served: Set(false),
                created_at: Set(now),
                updated_at: Set(now),
            }
            .insert(tx)
            .await?;
            announce(&call, "dns.zone.created", &row)?;
            apply_after_commit(&call, &row)?;
            Ok(row)
        })
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
            let current = load(tx, &params.id).await?;
            let now = call.require::<dyn Clock>()?.now();
            let row = zone::ActiveModel {
                id: Set(current.id.clone()),
                serial: Set(serial(now, Some(current.serial))),
                refresh: Set(seconds(params.refresh)),
                retry: Set(seconds(params.retry)),
                expire: Set(seconds(params.expire)),
                minimum: Set(seconds(params.minimum)),
                state: Set(zone::STATE_PENDING.to_owned()),
                updated_at: Set(now),
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
    let row = row.clone();
    call.after_commit(move |ctx| async move { dns.apply(&ctx, &row).await });
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

fn serial(now: DateTime<Utc>, current: Option<i64>) -> i64 {
    let day = i64::from(now.year()) * 10_000 + i64::from(now.month()) * 100 + i64::from(now.day());
    let base = day * SERIAL_REVISIONS;
    current.map_or(base, |current| (current + 1).max(base))
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
        .ok_or_else(|| KeelError::not_found("keel.error.not_found"))
}

#[cfg(test)]
mod tests {
    use keel::sdk::chrono::TimeZone;

    use super::*;

    #[test]
    fn a_serial_starts_at_the_day_and_grows_by_one_within_it_and_jumps_to_a_later_day() {
        let day = Utc
            .with_ymd_and_hms(2026, 9, 24, 10, 0, 0)
            .single()
            .expect("a time");
        assert_eq!(serial(day, None), 2_026_092_400);
        assert_eq!(serial(day, Some(2_026_092_400)), 2_026_092_401);
        assert_eq!(serial(day, Some(2_026_092_499)), 2_026_092_500);
        assert_eq!(serial(day, Some(2_026_010_105)), 2_026_092_400);
    }
}
