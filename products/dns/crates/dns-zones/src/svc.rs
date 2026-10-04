use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::OnConflict;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder,
    Set, Statement,
};
use keel::sdk::serde_json;
use keel::sdk::svc::{
    LoginTarget, OpenRequest, Opened, Period, Service, ServiceAccount, ServiceStatus, login, peak,
};
use keel::sdk::system::LimitSource;
use keel::sdk::{ServiceKind, ServiceKindDef};

use crate::api::ZoneTemplates;
use crate::{LIMIT, account, service, switch, zone};

pub(crate) const KIND: &str = "dns.zones";
pub(crate) const TITLE: &str = "dns.service.zones.title";
pub(crate) const LANDING: &str = "/f/dns.zone.list";
pub(crate) const ZONE_LIMIT_MAX: u32 = 100_000;
const TEMPLATES: &str = "dns.template.options";
const TEMPLATE_UNKNOWN: &str = "dns.service.error.template";
const TEMPLATE_FIELD: &str = "params.template";
const SETTLE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('dns_service:' || $1, 0))";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "DnsZonesParams",
    rename_all = "camelCase"
)]
pub(crate) struct Params {
    #[schemars(required, range(min = 0, max = ZONE_LIMIT_MAX), extend("type" = ["integer", "null"], "x-ui" = json!({ "control": "slider" })))]
    zone_limit: Option<u32>,
    #[serde(default)]
    #[schemars(extend("x-keel" = json!({ "options": TEMPLATES }), "x-ui" = json!({ "control": "select" })))]
    template: Option<Id>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "DnsZonesState",
    rename_all = "camelCase"
)]
pub(crate) struct State {
    zones_used: u64,
}

pub(crate) struct Zones;

pub(crate) fn kind() -> ServiceKindDef {
    ServiceKindDef::new(Zones)
}

#[derive(Clone, Copy)]
struct Standing {
    service: Id,
    account: Id,
    held: bool,
    closed: bool,
}

impl Standing {
    fn of(service: &Service, held: bool, closed: bool) -> Self {
        Self {
            service: service.id,
            account: service.account_id,
            held,
            closed,
        }
    }
}

#[async_trait]
impl ServiceKind for Zones {
    const KIND: &'static str = KIND;
    const TITLE: &'static str = TITLE;
    const SCHEMA_VERSION: u32 = 1;
    type Params = Params;
    type State = State;
    type Account = ServiceAccount;

    async fn open(
        &self,
        call: &Call<()>,
        request: OpenRequest<ServiceAccount, Params>,
    ) -> Result<Opened<State>> {
        let account = request.account_id;
        refuse_unseen(call, account, &request.params).await?;
        let standing = Standing {
            service: request.id,
            account,
            held: false,
            closed: false,
        };
        record(call, standing, &request.params).await?;
        switch::later(call, account)?;
        Ok(Opened {
            state: used(call, account).await?,
        })
    }

    async fn suspend(&self, call: &Call<()>, service: &Service, _: Value) -> Result<()> {
        shift(call, Standing::of(service, true, false), service).await
    }

    async fn resume(&self, call: &Call<()>, service: &Service) -> Result<()> {
        shift(call, Standing::of(service, false, false), service).await
    }

    async fn close(&self, call: &Call<()>, service: &Service) -> Result<()> {
        shift(call, Standing::of(service, true, true), service).await
    }

    async fn set_params(
        &self,
        call: &Call<()>,
        service: &Service,
        params: Params,
    ) -> Result<State> {
        refuse_unseen(call, service.account_id, &params).await?;
        let held = service.status != ServiceStatus::Active;
        record(call, Standing::of(service, held, false), &params).await?;
        used(call, service.account_id).await
    }

    async fn stat(&self, call: &Call<()>, service: &Service, period: Period) -> Result<Value> {
        let zones_used = peak(call, LIMIT, service, period).await?;
        Ok(serde_json::to_value(State { zones_used })?)
    }

    async fn login(&self, call: &Call<()>, service: &Service) -> Result<LoginTarget> {
        login(call, service, LANDING).await
    }
}

async fn refuse_unseen(call: &Call<()>, account: Id, params: &Params) -> Result<()> {
    let Some(template) = &params.template else {
        return Ok(());
    };
    let known = call
        .require::<dyn ZoneTemplates>()?
        .exists(call.tx()?, &AccountScope::Own(account), template)
        .await?;
    if known {
        return Ok(());
    }
    Err(KeelError::validation(TEMPLATE_UNKNOWN)
        .field(TEMPLATE_FIELD)
        .param("template", template.to_string()))
}

async fn shift(call: &Call<()>, standing: Standing, service: &Service) -> Result<()> {
    let params: Params = serde_json::from_value(service.params.clone())?;
    record(call, standing, &params).await?;
    switch::later(call, standing.account)
}

async fn record(call: &Call<()>, standing: Standing, params: &Params) -> Result<()> {
    let tx = call.tx()?;
    let account = standing.account.to_string();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SETTLE,
        [account.as_str().into()],
    ))
    .await?;
    let now = call.require::<dyn Clock>()?.now();
    service::Entity::insert(service::ActiveModel {
        service_id: Set(standing.service.to_string()),
        account_id: Set(account.clone()),
        zone_limit: Set(params
            .zone_limit
            .map(|limit| i32::try_from(limit).unwrap_or(i32::MAX))),
        template: Set(params.template.as_ref().map(ToString::to_string)),
        held: Set(standing.held),
        closed: Set(standing.closed),
        updated_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(service::Column::ServiceId)
            .update_columns([
                service::Column::ZoneLimit,
                service::Column::Template,
                service::Column::Held,
                service::Column::Closed,
                service::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    let Some(leading) = service::Entity::find()
        .filter(service::Column::AccountId.eq(account.as_str()))
        .filter(service::Column::Closed.eq(false))
        .order_by_asc(service::Column::Held)
        .order_by_desc(service::Column::Seq)
        .one(tx)
        .await?
    else {
        return Ok(());
    };
    account::Entity::insert(account::ActiveModel {
        account_id: Set(account),
        template: Set(leading.template),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(account::Column::AccountId)
            .update_columns([account::Column::Template, account::Column::UpdatedAt])
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    let limit = leading
        .zone_limit
        .map(|limit| u64::from(limit.unsigned_abs()));
    call.require::<dyn LimitSource>()?
        .store(LIMIT, standing.account, limit, tx)
        .await
}

async fn used(call: &Call<()>, account: Id) -> Result<State> {
    let zones_used = zone::Entity::find()
        .filter(zone::Column::AccountId.eq(account.to_string()))
        .count(call.db())
        .await?;
    Ok(State { zones_used })
}
