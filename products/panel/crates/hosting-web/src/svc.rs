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

use crate::host::Turn;
use crate::{COUNT, LIMIT, lifecycle, service, site};

pub(crate) const KIND: &str = "hosting.sites";
pub(crate) const TITLE: &str = "hosting.site.service.title";
pub(crate) const LANDING: &str = "/f/hosting.site.list";
pub(crate) const SITE_LIMIT_MAX: u32 = 100_000;
const SETTLE: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('hosting_site_service:' || $1, 0))";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingSitesParams",
    rename_all = "camelCase"
)]
pub(crate) struct Params {
    #[schemars(required, range(min = 0, max = SITE_LIMIT_MAX), extend("type" = ["integer", "null"], "x-ui" = json!({ "control": "slider" })))]
    site_limit: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingSitesState",
    rename_all = "camelCase"
)]
pub(crate) struct State {
    sites_used: u64,
}

pub(crate) struct Sites {
    turn: Turn,
}

pub(crate) fn kind(turn: Turn) -> ServiceKindDef {
    ServiceKindDef::new(Sites { turn })
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

impl Sites {
    async fn shift(&self, call: &Call<()>, standing: Standing, service: &Service) -> Result<()> {
        let params: Params = serde_json::from_value(service.params.clone())?;
        record(call, standing, &params).await?;
        lifecycle::later(call, &self.turn, standing.account)
    }
}

#[async_trait]
impl ServiceKind for Sites {
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
        let standing = Standing {
            service: request.id,
            account,
            held: false,
            closed: false,
        };
        record(call, standing, &request.params).await?;
        lifecycle::later(call, &self.turn, account)?;
        Ok(Opened {
            state: used(call, account).await?,
        })
    }

    async fn suspend(&self, call: &Call<()>, service: &Service, _: Value) -> Result<()> {
        self.shift(call, Standing::of(service, true, false), service)
            .await
    }

    async fn resume(&self, call: &Call<()>, service: &Service) -> Result<()> {
        self.shift(call, Standing::of(service, false, false), service)
            .await
    }

    async fn close(&self, call: &Call<()>, service: &Service) -> Result<()> {
        self.shift(call, Standing::of(service, true, true), service)
            .await
    }

    async fn set_params(
        &self,
        call: &Call<()>,
        service: &Service,
        params: Params,
    ) -> Result<State> {
        let held = service.status != ServiceStatus::Active;
        record(call, Standing::of(service, held, false), &params).await?;
        used(call, service.account_id).await
    }

    async fn stat(&self, call: &Call<()>, service: &Service, period: Period) -> Result<Value> {
        let sites_used = peak(call, COUNT, service, period).await?;
        Ok(serde_json::to_value(State { sites_used })?)
    }

    async fn login(&self, call: &Call<()>, service: &Service) -> Result<LoginTarget> {
        login(call, service, LANDING).await
    }
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
    service::Entity::insert(service::ActiveModel {
        service_id: Set(standing.service.to_string()),
        account_id: Set(account.clone()),
        site_limit: Set(params
            .site_limit
            .map(|limit| i32::try_from(limit).unwrap_or(i32::MAX))),
        held: Set(standing.held),
        closed: Set(standing.closed),
        updated_at: Set(call.require::<dyn Clock>()?.now()),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(service::Column::ServiceId)
            .update_columns([
                service::Column::SiteLimit,
                service::Column::Held,
                service::Column::Closed,
                service::Column::UpdatedAt,
            ])
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    let Some(leading) = service::Entity::find()
        .filter(service::Column::AccountId.eq(account))
        .filter(service::Column::Closed.eq(false))
        .order_by_asc(service::Column::Held)
        .order_by_desc(service::Column::Seq)
        .one(tx)
        .await?
    else {
        return Ok(());
    };
    let limit = leading
        .site_limit
        .map(|limit| u64::from(limit.unsigned_abs()));
    call.require::<dyn LimitSource>()?
        .store(LIMIT, standing.account, limit, tx)
        .await
}

async fn used(call: &Call<()>, account: Id) -> Result<State> {
    let sites_used = site::Entity::find()
        .filter(site::Column::AccountId.eq(account.to_string()))
        .count(call.db())
        .await?;
    Ok(State { sites_used })
}
