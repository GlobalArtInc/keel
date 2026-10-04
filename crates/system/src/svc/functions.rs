use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict};
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait,
    QueryFilter, QuerySelect, Set, Statement,
};
pub use keel_sdk::svc::SUBJECT;
use keel_sdk::svc::{
    OpenParams, OpenRequest, Period, Service, ServiceAccount, ServiceAnswer, ServiceRef,
    ServiceStatus, SetParams, StatAnswer, StatParams, SuspendParams,
};
use keel_sdk::system::{AccountSource, PeerReach};
use keel_sdk::types::{Account, AccountState, ErrorCode, Via};
use keel_sdk::{ServiceKindDef, serde_json};

use super::entity::service;
use super::{RETENTION_DAYS, RETENTION_DAYS_KEY};

pub const OPEN: &str = "open";
pub const SUSPEND: &str = "suspend";
pub const RESUME: &str = "resume";
pub const CLOSE: &str = "close";
pub const SET_PARAMS: &str = "set_params";
pub const STAT: &str = "stat";
pub const LOGIN: &str = "login";
pub const VERBS: [&str; 7] = [OPEN, SUSPEND, RESUME, CLOSE, SET_PARAMS, STAT, LOGIN];
pub const ERROR_EXISTS: &str = "svc.error.exists";
pub const ERROR_NOT_FOUND: &str = "svc.error.not_found";
pub const ERROR_CLOSED: &str = "svc.error.closed";
pub const ERROR_PERIOD: &str = "svc.error.period";
pub const ERROR_STORED: &str = "svc.error.stored";
pub const EVENT_OPENED: &str = "svc.service.opened";
pub const EVENT_SUSPENDED: &str = "svc.service.suspended";
pub const EVENT_RESUMED: &str = "svc.service.resumed";
pub const EVENT_CLOSED: &str = "svc.service.closed";
pub const EVENT_UPDATED: &str = "svc.service.updated";
pub const ACCOUNT_UPSERT: &str = "keel.account.upsert";
pub const ACCOUNT_SUSPEND: &str = "keel.account.suspend";
pub const ACCOUNT_RESUME: &str = "keel.account.resume";
pub const LOCK_PREFIX: &str = "svc_account:";
const ACCOUNT_LOCK: &str = "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))";
const EXTERNAL_ID: &str = "externalId";
const ACCOUNT: &str = "account";

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
struct Changed<'a> {
    id: Id,
    kind: &'a str,
    external_id: &'a str,
    status: ServiceStatus,
}

pub fn name(kind: &str, verb: &str) -> String {
    format!("svc.{kind}.{verb}")
}

pub fn lock_key(origin: &str, external_id: &str) -> String {
    format!("{LOCK_PREFIX}{origin} {external_id}")
}

pub(super) fn all(def: &ServiceKindDef) -> [FunctionDef; 7] {
    [
        open(def.clone()),
        suspend(def.clone()),
        resume(def.clone()),
        close(def.clone()),
        set_params(def.clone()),
        stat(def.clone()),
        login(def.clone()),
    ]
}

fn open(def: ServiceKindDef) -> FunctionDef {
    let params = def.schemas().open.clone();
    let result = def.schemas().answer.clone();
    function::<OpenParams<Value, Value>, ServiceAnswer<Value>>(&name(def.kind(), OPEN))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .idempotent(Idempotency::Required)
        .handler(move |call: Call<OpenParams<Value, Value>>| {
            let def = def.clone();
            async move { opened(&call, &def).await }
        })
        .with_params(params)
        .with_result(result)
}

fn suspend(def: ServiceKindDef) -> FunctionDef {
    let result = def.schemas().answer.clone();
    function::<SuspendParams, ServiceAnswer<Value>>(&name(def.kind(), SUSPEND))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<SuspendParams>| {
            let def = def.clone();
            async move { suspended(&call, &def).await }
        })
        .with_result(result)
}

fn resume(def: ServiceKindDef) -> FunctionDef {
    let result = def.schemas().answer.clone();
    function::<ServiceRef, ServiceAnswer<Value>>(&name(def.kind(), RESUME))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<ServiceRef>| {
            let def = def.clone();
            async move { resumed(&call, &def).await }
        })
        .with_result(result)
}

fn close(def: ServiceKindDef) -> FunctionDef {
    let result = def.schemas().answer.clone();
    function::<ServiceRef, ServiceAnswer<Value>>(&name(def.kind(), CLOSE))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .idempotent(Idempotency::Required)
        .handler(move |call: Call<ServiceRef>| {
            let def = def.clone();
            async move { closed(&call, &def).await }
        })
        .with_result(result)
}

fn set_params(def: ServiceKindDef) -> FunctionDef {
    let params = def.schemas().set_params.clone();
    let result = def.schemas().answer.clone();
    function::<SetParams<Value>, ServiceAnswer<Value>>(&name(def.kind(), SET_PARAMS))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<SetParams<Value>>| {
            let def = def.clone();
            async move { changed(&call, &def).await }
        })
        .with_params(params)
        .with_result(result)
}

fn stat(def: ServiceKindDef) -> FunctionDef {
    function::<StatParams, StatAnswer>(&name(def.kind(), STAT))
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<StatParams>| {
            let def = def.clone();
            async move { measured(&call, &def).await }
        })
}

fn login(def: ServiceKindDef) -> FunctionDef {
    function::<ServiceRef, keel_sdk::svc::LoginTarget>(&name(def.kind(), LOGIN))
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .handler(move |call: Call<ServiceRef>| {
            let def = def.clone();
            async move {
                let row = locked(&call, &def, &call.params().external_id).await?;
                let service = service_of(&row)?;
                refuse_closed(&def, &service)?;
                def.login(&call, &service).await
            }
        })
}

async fn opened(
    call: &Call<OpenParams<Value, Value>>,
    def: &ServiceKindDef,
) -> Result<ServiceAnswer<Value>> {
    let params = call.params();
    let identity: ServiceAccount =
        serde_json::from_value(params.account.clone()).map_err(|error| {
            KeelError::validation("keel.error.validation")
                .field(ACCOUNT)
                .source(error)
        })?;
    lock_account(call, &identity.origin, &identity.external_id).await?;
    let account_id = upsert_account(call, &identity).await?;
    reopen_account(call, account_id).await?;
    let tx = call.tx()?;
    let id = Id::generate();
    let now = call.require::<dyn Clock>()?.now();
    let inserted = service::Entity::insert(service::ActiveModel {
        id: Set(id.to_string()),
        kind: Set(def.kind().to_owned()),
        external_id: Set(params.external_id.clone()),
        account_id: Set(account_id.to_string()),
        status: Set(ServiceStatus::Active.as_str().to_owned()),
        params_json: Set(params.params.clone()),
        state_json: Set(Value::Null),
        schema_version: Set(version(def)?),
        peer_id: Set(peer_of(call.principal()).map(|link| link.to_string())),
        retain_until: Set(None),
        suspended_account: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([service::Column::Kind, service::Column::ExternalId])
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(tx)
    .await?;
    if inserted == 0 {
        return Err(KeelError::conflict(ERROR_EXISTS)
            .param("kind", def.kind())
            .param(EXTERNAL_ID, params.external_id.as_str()));
    }
    let opened = def
        .open(
            call,
            OpenRequest {
                id,
                external_id: params.external_id.clone(),
                account_id,
                account: params.account.clone(),
                params: params.params.clone(),
            },
        )
        .await?;
    service::ActiveModel {
        id: Set(id.to_string()),
        state_json: Set(opened.state.clone()),
        ..Default::default()
    }
    .update(tx)
    .await?;
    emit(
        call,
        EVENT_OPENED,
        def,
        id,
        account_id,
        &params.external_id,
        ServiceStatus::Active,
    )?;
    Ok(ServiceAnswer {
        service_id: id,
        account_id,
        status: ServiceStatus::Active,
        state: opened.state,
        retain_until: None,
    })
}

async fn upsert_account<P>(call: &Call<P>, identity: &ServiceAccount) -> Result<Id> {
    let mut account = json!({ "origin": identity.origin, EXTERNAL_ID: identity.external_id });
    if let Some(kind) = &identity.kind {
        account["kind"] = json!(kind);
    }
    let upserted = call
        .invoke(ACCOUNT_UPSERT, account)
        .await
        .map_err(under_account)?;
    upserted["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| {
            KeelError::new(ErrorCode::Internal)
                .param("function", ACCOUNT_UPSERT)
                .param("id", upserted["id"].clone())
        })
}

async fn reopen_account<P>(call: &Call<P>, account_id: Id) -> Result<()> {
    let tx = call.tx()?;
    let marked = service::Entity::find()
        .filter(service::Column::AccountId.eq(account_id.to_string()))
        .filter(service::Column::SuspendedAccount.eq(true))
        .count(tx)
        .await?;
    if marked == 0 {
        return Ok(());
    }
    let suspended = call
        .require::<dyn AccountSource>()?
        .by_id(account_id, tx)
        .await?
        .is_some_and(|account| account.state == AccountState::Suspended);
    if suspended {
        call.invoke(ACCOUNT_RESUME, json!({ "id": account_id }))
            .await?;
    }
    Ok(())
}

async fn suspend_account<P>(call: &Call<P>, account_id: Id) -> Result<bool> {
    let switched = call
        .invoke(ACCOUNT_SUSPEND, json!({ "id": account_id }))
        .await?;
    switched["changed"].as_bool().ok_or_else(|| {
        KeelError::new(ErrorCode::Internal)
            .param("function", ACCOUNT_SUSPEND)
            .param("changed", switched["changed"].clone())
    })
}

pub(super) async fn unmarked(call: Call<Value>) -> Result<()> {
    let Some(account_id) = call
        .params()
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(());
    };
    let tx = call.tx()?;
    let Some(account) = call
        .require::<dyn AccountSource>()?
        .by_id(account_id, tx)
        .await?
    else {
        return Ok(());
    };
    let (Some(origin), Some(external_id)) = (&account.origin, &account.external_id) else {
        return Ok(());
    };
    lock_account(&call, origin, external_id).await?;
    service::Entity::update_many()
        .col_expr(service::Column::SuspendedAccount, Expr::value(false))
        .filter(service::Column::AccountId.eq(account_id.to_string()))
        .filter(service::Column::SuspendedAccount.eq(true))
        .exec(tx)
        .await?;
    Ok(())
}

fn under_account(error: KeelError) -> KeelError {
    let mut wire = error.to_call_error();
    if wire.fields.is_empty() {
        return error;
    }
    for field in &mut wire.fields {
        field.path = format!("{ACCOUNT}.{}", field.path);
    }
    KeelError::from(wire)
}

async fn suspended(
    call: &Call<SuspendParams>,
    def: &ServiceKindDef,
) -> Result<ServiceAnswer<Value>> {
    let params = call.params();
    let row = locked(call, def, &params.external_id).await?;
    let service = service_of(&row)?;
    refuse_closed(def, &service)?;
    if service.status == ServiceStatus::Suspended {
        return Ok(answer(&service, None));
    }
    def.suspend(call, &service, params.reason.clone()).await?;
    switch(
        call,
        def,
        service,
        ServiceStatus::Suspended,
        EVENT_SUSPENDED,
    )
    .await
}

async fn resumed(call: &Call<ServiceRef>, def: &ServiceKindDef) -> Result<ServiceAnswer<Value>> {
    let row = locked(call, def, &call.params().external_id).await?;
    let service = service_of(&row)?;
    refuse_closed(def, &service)?;
    if service.status == ServiceStatus::Active {
        return Ok(answer(&service, None));
    }
    def.resume(call, &service).await?;
    switch(call, def, service, ServiceStatus::Active, EVENT_RESUMED).await
}

async fn switch<P>(
    call: &Call<P>,
    def: &ServiceKindDef,
    mut service: Service,
    to: ServiceStatus,
    event: &str,
) -> Result<ServiceAnswer<Value>> {
    service::ActiveModel {
        id: Set(service.id.to_string()),
        status: Set(to.as_str().to_owned()),
        updated_at: Set(call.require::<dyn Clock>()?.now()),
        ..Default::default()
    }
    .update(call.tx()?)
    .await?;
    service.status = to;
    emit(
        call,
        event,
        def,
        service.id,
        service.account_id,
        &service.external_id,
        to,
    )?;
    Ok(answer(&service, None))
}

async fn closed(call: &Call<ServiceRef>, def: &ServiceKindDef) -> Result<ServiceAnswer<Value>> {
    let external_id = &call.params().external_id;
    let (found, account) = find(call, def, external_id).await?;
    let (Some(origin), Some(identity)) = (&account.origin, &account.external_id) else {
        return Err(stored(&found, "accountId"));
    };
    lock_account(call, origin, identity).await?;
    let row = service::Entity::find_by_id(found.id.clone())
        .lock_exclusive()
        .one(call.tx()?)
        .await?
        .ok_or_else(|| not_found(def, external_id))?;
    let mut service = service_of(&row)?;
    refuse_closed(def, &service)?;
    def.close(call, &service).await?;
    let now = call.require::<dyn Clock>()?.now();
    let retain_until = now + Duration::days(i64::from(retention_days(call).await?));
    let tx = call.tx()?;
    let others = service::Entity::find()
        .filter(service::Column::AccountId.eq(service.account_id.to_string()))
        .filter(service::Column::Id.ne(service.id.to_string()))
        .filter(service::Column::Status.ne(ServiceStatus::Closed.as_str()))
        .count(tx)
        .await?;
    let suspended_account = others == 0 && suspend_account(call, service.account_id).await?;
    service::ActiveModel {
        id: Set(service.id.to_string()),
        status: Set(ServiceStatus::Closed.as_str().to_owned()),
        retain_until: Set(Some(retain_until)),
        suspended_account: Set(suspended_account),
        updated_at: Set(now),
        ..Default::default()
    }
    .update(tx)
    .await?;
    service.status = ServiceStatus::Closed;
    emit(
        call,
        EVENT_CLOSED,
        def,
        service.id,
        service.account_id,
        &service.external_id,
        ServiceStatus::Closed,
    )?;
    Ok(answer(&service, Some(retain_until)))
}

async fn retention_days<P>(call: &Call<P>) -> Result<u32> {
    Ok(call
        .setting::<Option<u32>>(RETENTION_DAYS_KEY)
        .await?
        .unwrap_or(RETENTION_DAYS))
}

async fn lock_account<P>(call: &Call<P>, origin: &str, external_id: &str) -> Result<()> {
    call.tx()?
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            ACCOUNT_LOCK,
            [lock_key(origin, external_id).into()],
        ))
        .await?;
    Ok(())
}

async fn changed(
    call: &Call<SetParams<Value>>,
    def: &ServiceKindDef,
) -> Result<ServiceAnswer<Value>> {
    let params = call.params();
    let row = locked(call, def, &params.external_id).await?;
    let mut service = service_of(&row)?;
    refuse_closed(def, &service)?;
    let state = def
        .set_params(call, &service, params.params.clone())
        .await?;
    service::ActiveModel {
        id: Set(service.id.to_string()),
        params_json: Set(params.params.clone()),
        state_json: Set(state.clone()),
        schema_version: Set(version(def)?),
        updated_at: Set(call.require::<dyn Clock>()?.now()),
        ..Default::default()
    }
    .update(call.tx()?)
    .await?;
    service.state = state;
    emit(
        call,
        EVENT_UPDATED,
        def,
        service.id,
        service.account_id,
        &service.external_id,
        service.status,
    )?;
    Ok(answer(&service, None))
}

async fn measured(call: &Call<StatParams>, def: &ServiceKindDef) -> Result<StatAnswer> {
    let params = call.params();
    if params.to <= params.from {
        return Err(KeelError::validation(ERROR_PERIOD).field("to"));
    }
    let (row, _) = find(call, def, &params.external_id).await?;
    let service = service_of(&row)?;
    let period = Period {
        from: params.from,
        to: params.to,
    };
    let usage = def.stat(call, &service, period).await?;
    Ok(StatAnswer {
        service_id: service.id,
        from: params.from,
        to: params.to,
        usage,
    })
}

async fn find<P>(
    call: &Call<P>,
    def: &ServiceKindDef,
    external_id: &str,
) -> Result<(service::Model, Account)> {
    let row = service::Entity::find()
        .filter(service::Column::Kind.eq(def.kind()))
        .filter(service::Column::ExternalId.eq(external_id))
        .one(call.db())
        .await?;
    within_reach(call, def, external_id, row).await
}

async fn locked<P>(
    call: &Call<P>,
    def: &ServiceKindDef,
    external_id: &str,
) -> Result<service::Model> {
    let row = service::Entity::find()
        .filter(service::Column::Kind.eq(def.kind()))
        .filter(service::Column::ExternalId.eq(external_id))
        .lock_exclusive()
        .one(call.tx()?)
        .await?;
    Ok(within_reach(call, def, external_id, row).await?.0)
}

async fn within_reach<P>(
    call: &Call<P>,
    def: &ServiceKindDef,
    external_id: &str,
    row: Option<service::Model>,
) -> Result<(service::Model, Account)> {
    let row = row.ok_or_else(|| not_found(def, external_id))?;
    let account_id = row
        .account_id
        .parse()
        .map_err(|_| stored(&row, "accountId"))?;
    let account = call
        .require::<dyn AccountSource>()?
        .by_id(account_id, call.db())
        .await?
        .ok_or_else(|| stored(&row, "accountId"))?;
    let admitted = match call.require::<dyn PeerReach>() {
        Ok(reach) => {
            reach
                .admits(call.principal(), account.origin.as_deref(), call.db())
                .await?
        }
        Err(_) => true,
    };
    if !admitted {
        return Err(not_found(def, external_id));
    }
    Ok((row, account))
}

fn not_found(def: &ServiceKindDef, external_id: &str) -> KeelError {
    KeelError::not_found(ERROR_NOT_FOUND)
        .param("kind", def.kind())
        .param(EXTERNAL_ID, external_id)
}

fn refuse_closed(def: &ServiceKindDef, service: &Service) -> Result<()> {
    if service.status == ServiceStatus::Closed {
        return Err(KeelError::conflict(ERROR_CLOSED)
            .param("kind", def.kind())
            .param(EXTERNAL_ID, service.external_id.as_str()));
    }
    Ok(())
}

fn stored(row: &service::Model, member: &str) -> KeelError {
    KeelError::internal(ERROR_STORED)
        .param("id", row.id.as_str())
        .param("member", member)
}

pub(super) fn service_of(row: &service::Model) -> Result<Service> {
    let stored = |member: &str| stored(row, member);
    let peer_id = match &row.peer_id {
        None => None,
        Some(link) => Some(link.parse().map_err(|_| stored("peerId"))?),
    };
    Ok(Service {
        id: row.id.parse().map_err(|_| stored("id"))?,
        kind: row.kind.clone(),
        external_id: row.external_id.clone(),
        account_id: row.account_id.parse().map_err(|_| stored("accountId"))?,
        status: ServiceStatus::parse(&row.status).ok_or_else(|| stored("status"))?,
        params: row.params_json.clone(),
        state: row.state_json.clone(),
        schema_version: u32::try_from(row.schema_version).map_err(|_| stored("schemaVersion"))?,
        peer_id,
    })
}

fn answer(service: &Service, retain_until: Option<DateTime<Utc>>) -> ServiceAnswer<Value> {
    ServiceAnswer {
        service_id: service.id,
        account_id: service.account_id,
        status: service.status,
        state: service.state.clone(),
        retain_until,
    }
}

fn version(def: &ServiceKindDef) -> Result<i32> {
    i32::try_from(def.schema_version()).map_err(|_| {
        KeelError::internal(super::ERROR_KIND)
            .param("kind", def.kind())
            .param("reason", "the schema version does not fit the table")
    })
}

pub(super) fn peer_of(principal: &Principal) -> Option<Id> {
    match principal.via {
        Via::Peer(link) => Some(link),
        _ => None,
    }
}

fn emit<P>(
    call: &Call<P>,
    event: &str,
    def: &ServiceKindDef,
    id: Id,
    account_id: Id,
    external_id: &str,
    status: ServiceStatus,
) -> Result<()> {
    call.emit_about(
        event,
        EventSubject {
            kind: SUBJECT.to_owned(),
            id,
            account_id: Some(account_id),
        },
        &Changed {
            id,
            kind: def.kind(),
            external_id,
            status,
        },
    )
}
