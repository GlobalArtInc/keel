use keel::prelude::*;
use keel::sdk::escape_like;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::sea_query::extension::postgres::PgExpr;
use keel::sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use keel_hosting_web::api::{DnsProvider, SiteZone, ZoneRecord};

use crate::host::{Changed, Mail};
use crate::{FEATURE, MANAGE, READ, STATE_ERROR, STATE_PENDING, domain, mailbox, names};

const LIST: &str = "hosting.mail.domain.list";
pub(crate) const CREATE: &str = "hosting.mail.domain.create";
const DELETE: &str = "hosting.mail.domain.delete";
pub(crate) const OPTIONS: &str = "hosting.mail.domain.options";
const MAX_OPTIONS: u32 = 100;
const EXISTS: &str = "hosting.mail.domain.error.exists";
const FOREIGN_ZONE: &str = "hosting.mail.domain.error.foreign_zone";
const MX_PRIORITY: u16 = 10;
const SPF: &str = "\"v=spf1 mx -all\"";
const APEX: &str = "@";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct DomainCreate {
    #[schemars(length(min = 3, max = 253), extend("x-keel" = json!({ "check": "domain" })))]
    name: String,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Batch {
    #[schemars(length(min = 1))]
    pub(crate) ids: Vec<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Deleted {
    pub(crate) deleted: u64,
}

pub(crate) fn list() -> FunctionDef {
    function::<ListQuery, Page<domain::Model>>(LIST)
        .kind(Kind::List)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<domain::Entity>()
        .ui(List::new("hosting.mail.domain.list.title")
            .search(["name"])
            .column(Column::new("name", ColumnType::Text).sortable().primary())
            .column(state_column())
            .column(Column::new("error", ColumnType::Text))
            .column(
                Column::new("accountId", ColumnType::Text)
                    .level(Level::Reseller)
                    .filter_remote("keel.account.options"),
            )
            .action(
                Action::new("create", ActionRole::New)
                    .screen(CREATE)
                    .icon("plus"),
            )
            .action(
                Action::new("delete", ActionRole::Delete)
                    .function(DELETE)
                    .confirm_with(ConfirmDetails {
                        destructive: true,
                        items: Some("name".to_owned()),
                        input: Some(ConfirmInput::Count),
                    }),
            )
            .row_actions(["delete"])
            .empty("hosting.mail.domain.empty", Some("create"))
            .invalidate_on(["hosting.mail.*"]))
        .default_sort("name", SortDir::Asc)
        .source(entity_source::<domain::Entity>())
}

pub(crate) fn state_column() -> Column {
    Column::new("state", ColumnType::Status)
        .map([
            (STATE_PENDING, Tone::Neutral),
            (crate::STATE_ACTIVE, Tone::Success),
            (STATE_ERROR, Tone::Danger),
        ])
        .filter_options([STATE_PENDING, crate::STATE_ACTIVE, STATE_ERROR])
}

pub(crate) fn create(mail: Arc<Mail>) -> FunctionDef {
    function::<DomainCreate, domain::Model>(CREATE)
        .kind(Kind::Form)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .owned::<domain::Entity>()
        .ui(Form::new("hosting.mail.domain.create.title")
            .page("main", ["name"])
            .page("owner", ["account"])
            .show_if("$principal.level != 'user'"))
        .handler(move |call: Call<DomainCreate>| {
            let mail = Arc::clone(&mail);
            async move {
                let params = call.params();
                let tx = call.tx()?;
                let account = call.resolve_account(params.account).await?;
                let name = names::domain(&params.name)?;
                refuse_taken(tx, &name).await?;
                let zone = SiteZone {
                    name: name.clone(),
                    account,
                    own_account: account == call.principal().account_id,
                };
                let invoke = |func: &str, params: Value| call.invoke(func, params);
                let dns = call.require::<dyn DnsProvider>()?;
                if dns.foreign(&invoke, &zone).await? {
                    return Err(KeelError::validation(FOREIGN_ZONE)
                        .field("name")
                        .param("name", name));
                }
                let now = call.require::<dyn Clock>()?.now();
                let row = domain::ActiveModel {
                    id: Set(Id::generate().to_string()),
                    account_id: Set(account.to_string()),
                    name: Set(name.clone()),
                    state: Set(STATE_PENDING.to_owned()),
                    error: Set(None),
                    created_at: Set(now),
                    updated_at: Set(now),
                }
                .insert(tx)
                .await?;
                dns.add_records(&invoke, &zone, &mail_records(&name))
                    .await?;
                let changed = Changed {
                    domains: vec![row.id.clone()],
                    ..Changed::default()
                };
                call.after_commit(move |ctx| async move { mail.apply(&ctx, changed).await });
                Ok(row)
            }
        })
}

pub(crate) fn delete(mail: Arc<Mail>) -> FunctionDef {
    function::<Batch, Deleted>(DELETE)
        .kind(Kind::Action)
        .access(Level::User, MANAGE)
        .feature(FEATURE)
        .key("ids")
        .owned::<domain::Entity>()
        .handler(move |call: Call<Batch>| {
            let mail = Arc::clone(&mail);
            async move {
                let ids: Vec<String> = call.params().ids.iter().map(ToString::to_string).collect();
                let (deleted, gone) = drop_rows(call.tx()?, domain::Column::Id.is_in(ids)).await?;
                apply_after_commit(&call, mail, gone);
                Ok(Deleted { deleted })
            }
        })
}

pub(crate) fn options() -> FunctionDef {
    function::<OptionsQuery, Choices>(OPTIONS)
        .kind(Kind::Options)
        .access(Level::User, READ)
        .feature(FEATURE)
        .unowned()
        .handler(|call: Call<OptionsQuery>| async move {
            let query = call.params();
            let limit = query.limit.clamp(1, MAX_OPTIONS);
            let mut select = call.account_scope().apply(domain::Entity::find());
            if !query.search.is_empty() {
                let pattern = format!("%{}%", escape_like(&query.search));
                select = select.filter(Expr::col(domain::Column::Name).ilike(pattern.as_str()));
            }
            if let Some(cursor) = &query.cursor {
                select = select.filter(domain::Column::Id.gt(cursor.as_str()));
            }
            let mut rows = select
                .order_by_asc(domain::Column::Id)
                .limit(u64::from(limit) + 1)
                .all(call.db())
                .await?;
            let more = rows.len() > limit as usize;
            rows.truncate(limit as usize);
            Ok(Choices {
                data: rows
                    .into_iter()
                    .map(|row| Choice::new(row.id, row.name))
                    .collect(),
                more,
            })
        })
}

pub(crate) async fn drop_rows<C: ConnectionTrait>(
    db: &C,
    filter: impl keel::sdk::sea_orm::sea_query::IntoCondition,
) -> Result<(u64, Vec<String>)> {
    let condition = filter.into_condition();
    let domains: Vec<String> = domain::Entity::find()
        .filter(condition.clone())
        .all(db)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if domains.is_empty() {
        return Ok((0, Vec::new()));
    }
    let gone =
        crate::mailboxes::drop_rows(db, mailbox::Column::DomainId.is_in(domains.clone())).await?;
    let deleted = domain::Entity::delete_many()
        .filter(domain::Column::Id.is_in(domains))
        .exec(db)
        .await?
        .rows_affected;
    Ok((deleted, gone))
}

pub(crate) fn apply_after_commit<P>(call: &Call<P>, mail: Arc<Mail>, gone: Vec<String>) {
    let changed = Changed {
        gone,
        ..Changed::default()
    };
    call.after_commit(move |ctx| async move { mail.apply(&ctx, changed).await });
}

fn mail_records(domain: &str) -> [ZoneRecord; 2] {
    [
        ZoneRecord {
            kind: "MX",
            name: APEX.to_owned(),
            value: domain.to_owned(),
            priority: Some(MX_PRIORITY),
        },
        ZoneRecord {
            kind: "TXT",
            name: APEX.to_owned(),
            value: SPF.to_owned(),
            priority: None,
        },
    ]
}

async fn refuse_taken<C: ConnectionTrait>(db: &C, name: &str) -> Result<()> {
    let taken = domain::Entity::find()
        .filter(domain::Column::Name.eq(name))
        .one(db)
        .await?;
    match taken {
        Some(_) => Err(KeelError::conflict(EXISTS)
            .field("name")
            .param("name", name)),
        None => Ok(()),
    }
}
