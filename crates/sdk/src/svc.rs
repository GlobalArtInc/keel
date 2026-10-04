use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use futures::future::BoxFuture;
use keel_types::{EXTERNAL_MAX_LEN, EXTERNAL_PATTERN, Id};
use schemars::{JsonSchema, Schema, schema_for};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::Call;
use crate::error::{KeelError, Result};
use crate::system::{AccountSource, PrincipalSource};

const VALIDATION: &str = "keel.error.validation";
const KEY_ISSUE: &str = "auth.key.issue";
const SERIES: &str = "stats.series";
const SERIES_POINTS: &str = "stats.error.points";
const GRAINS: [(&str, Option<i64>); 3] =
    [("day", Some(86_400)), ("hour", Some(3_600)), ("raw", None)];
pub const SUBJECT: &str = "svc.service";
pub const LOGIN_TTL_SECONDS: u32 = 60;
pub const ERROR_NO_PRINCIPAL: &str = "svc.error.no_principal";
pub const ERROR_ANSWER: &str = "svc.error.answer";

#[async_trait]
pub trait ServiceKind: Send + Sync + 'static {
    const KIND: &'static str;
    const TITLE: &'static str;
    const SCHEMA_VERSION: u32;
    type Params: JsonSchema + DeserializeOwned + Serialize + Send;
    type State: JsonSchema + Serialize + Send;
    type Account: JsonSchema + DeserializeOwned + Send;

    async fn open(
        &self,
        call: &Call<()>,
        request: OpenRequest<Self::Account, Self::Params>,
    ) -> Result<Opened<Self::State>>;
    async fn suspend(&self, call: &Call<()>, service: &Service, reason: Value) -> Result<()>;
    async fn resume(&self, call: &Call<()>, service: &Service) -> Result<()>;
    async fn close(&self, call: &Call<()>, service: &Service) -> Result<()>;
    async fn set_params(
        &self,
        call: &Call<()>,
        service: &Service,
        params: Self::Params,
    ) -> Result<Self::State>;
    async fn stat(&self, call: &Call<()>, service: &Service, period: Period) -> Result<Value>;
    async fn login(&self, call: &Call<()>, service: &Service) -> Result<LoginTarget>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStatus {
    Active,
    Suspended,
    Closed,
}

impl ServiceStatus {
    pub const ALL: [ServiceStatus; 3] = [
        ServiceStatus::Active,
        ServiceStatus::Suspended,
        ServiceStatus::Closed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ServiceStatus::Active => "active",
            ServiceStatus::Suspended => "suspended",
            ServiceStatus::Closed => "closed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.as_str() == text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct ServiceAccount {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub origin: String,
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-keel" = json!({ "check": "name" })))]
    pub kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenRequest<A, P> {
    pub id: Id,
    pub external_id: String,
    pub account_id: Id,
    pub account: A,
    pub params: P,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Opened<S> {
    pub state: S,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    pub id: Id,
    pub kind: String,
    pub external_id: String,
    pub account_id: Id,
    pub status: ServiceStatus,
    pub params: Value,
    pub state: Value,
    pub schema_version: u32,
    pub peer_id: Option<Id>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct Period {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct LoginTarget {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct OpenParams<A, P> {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    pub account: A,
    pub params: P,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct ServiceRef {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct SuspendParams {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    #[serde(default)]
    pub reason: Value,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct SetParams<P> {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    pub params: P,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct StatParams {
    #[schemars(length(min = 1, max = EXTERNAL_MAX_LEN), regex(pattern = EXTERNAL_PATTERN))]
    pub external_id: String,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct ServiceAnswer<S> {
    pub service_id: Id,
    pub account_id: Id,
    pub status: ServiceStatus,
    pub state: S,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename_all = "camelCase")]
pub struct StatAnswer {
    pub service_id: Id,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub usage: Value,
}

#[derive(Clone, Debug)]
pub struct ServiceSchemas {
    pub account: Schema,
    pub params: Schema,
    pub state: Schema,
    pub open: Schema,
    pub set_params: Schema,
    pub answer: Schema,
}

#[derive(Clone)]
pub struct ServiceKindDef {
    kind: &'static str,
    title: &'static str,
    schema_version: u32,
    schemas: Arc<ServiceSchemas>,
    handler: Arc<dyn Erased>,
}

impl ServiceKindDef {
    pub fn new<K: ServiceKind>(kind: K) -> Self {
        Self {
            kind: K::KIND,
            title: K::TITLE,
            schema_version: K::SCHEMA_VERSION,
            schemas: Arc::new(ServiceSchemas {
                account: schema_for!(K::Account),
                params: schema_for!(K::Params),
                state: schema_for!(K::State),
                open: schema_for!(OpenParams<K::Account, K::Params>),
                set_params: schema_for!(SetParams<K::Params>),
                answer: schema_for!(ServiceAnswer<K::State>),
            }),
            handler: Arc::new(Typed(kind)),
        }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn title(&self) -> &'static str {
        self.title
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn schemas(&self) -> &ServiceSchemas {
        &self.schemas
    }

    pub fn open<'a, P>(
        &'a self,
        call: &Call<P>,
        request: OpenRequest<Value, Value>,
    ) -> BoxFuture<'a, Result<Opened<Value>>> {
        self.handler.open(call.unit(), request)
    }

    pub fn suspend<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
        reason: Value,
    ) -> BoxFuture<'a, Result<()>> {
        self.handler.suspend(call.unit(), service, reason)
    }

    pub fn resume<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
    ) -> BoxFuture<'a, Result<()>> {
        self.handler.resume(call.unit(), service)
    }

    pub fn close<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
    ) -> BoxFuture<'a, Result<()>> {
        self.handler.close(call.unit(), service)
    }

    pub fn set_params<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
        params: Value,
    ) -> BoxFuture<'a, Result<Value>> {
        self.handler.set_params(call.unit(), service, params)
    }

    pub fn stat<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
        period: Period,
    ) -> BoxFuture<'a, Result<Value>> {
        self.handler.stat(call.unit(), service, period)
    }

    pub fn login<'a, P>(
        &'a self,
        call: &Call<P>,
        service: &'a Service,
    ) -> BoxFuture<'a, Result<LoginTarget>> {
        self.handler.login(call.unit(), service)
    }
}

impl fmt::Debug for ServiceKindDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceKindDef")
            .field("kind", &self.kind)
            .field("title", &self.title)
            .field("schema_version", &self.schema_version)
            .finish()
    }
}

trait Erased: Send + Sync {
    fn open<'a>(
        &'a self,
        call: Call<()>,
        request: OpenRequest<Value, Value>,
    ) -> BoxFuture<'a, Result<Opened<Value>>>;
    fn suspend<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        reason: Value,
    ) -> BoxFuture<'a, Result<()>>;
    fn resume<'a>(&'a self, call: Call<()>, service: &'a Service) -> BoxFuture<'a, Result<()>>;
    fn close<'a>(&'a self, call: Call<()>, service: &'a Service) -> BoxFuture<'a, Result<()>>;
    fn set_params<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        params: Value,
    ) -> BoxFuture<'a, Result<Value>>;
    fn stat<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        period: Period,
    ) -> BoxFuture<'a, Result<Value>>;
    fn login<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
    ) -> BoxFuture<'a, Result<LoginTarget>>;
}

struct Typed<K>(K);

fn typed<T: DeserializeOwned>(value: Value, field: &str) -> Result<T> {
    serde_json::from_value(value)
        .map_err(|error| KeelError::validation(VALIDATION).field(field).source(error))
}

impl<K: ServiceKind> Erased for Typed<K> {
    fn open<'a>(
        &'a self,
        call: Call<()>,
        request: OpenRequest<Value, Value>,
    ) -> BoxFuture<'a, Result<Opened<Value>>> {
        Box::pin(async move {
            let request = OpenRequest {
                id: request.id,
                external_id: request.external_id,
                account_id: request.account_id,
                account: typed::<K::Account>(request.account, "account")?,
                params: typed::<K::Params>(request.params, "params")?,
            };
            let opened = self.0.open(&call, request).await?;
            Ok(Opened {
                state: serde_json::to_value(opened.state)?,
            })
        })
    }

    fn suspend<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        reason: Value,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.0.suspend(&call, service, reason).await })
    }

    fn resume<'a>(&'a self, call: Call<()>, service: &'a Service) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.0.resume(&call, service).await })
    }

    fn close<'a>(&'a self, call: Call<()>, service: &'a Service) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.0.close(&call, service).await })
    }

    fn set_params<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        params: Value,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let params = typed::<K::Params>(params, "params")?;
            let state = self.0.set_params(&call, service, params).await?;
            Ok(serde_json::to_value(state)?)
        })
    }

    fn stat<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
        period: Period,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move { self.0.stat(&call, service, period).await })
    }

    fn login<'a>(
        &'a self,
        call: Call<()>,
        service: &'a Service,
    ) -> BoxFuture<'a, Result<LoginTarget>> {
        Box::pin(async move { self.0.login(&call, service).await })
    }
}

pub async fn login<P>(call: &Call<P>, service: &Service, redirect: &str) -> Result<LoginTarget> {
    let db = call.db();
    let origin = call
        .require::<dyn AccountSource>()?
        .by_id(service.account_id, db)
        .await?
        .and_then(|account| account.origin);
    let principal = call
        .require::<dyn PrincipalSource>()?
        .of_account(service.account_id, db)
        .await?
        .into_iter()
        .find(|principal| principal.origin == origin)
        .ok_or_else(|| {
            KeelError::conflict(ERROR_NO_PRINCIPAL)
                .param("accountId", service.account_id.to_string())
        })?;
    let issued = call
        .invoke(
            KEY_ISSUE,
            json!({ "principal": principal.id, "ttl": LOGIN_TTL_SECONDS, "redirect": redirect }),
        )
        .await?;
    let url = issued["url"]
        .as_str()
        .ok_or_else(|| unexpected(KEY_ISSUE, "url"))?;
    Ok(LoginTarget {
        url: url.to_owned(),
    })
}

pub async fn peak<P>(call: &Call<P>, key: &str, service: &Service, period: Period) -> Result<u64> {
    let series = Series {
        key,
        subject: service.account_id,
    };
    let mut peak = 0.0_f64;
    let mut spans = vec![(0, period.from, period.to)];
    while let Some((grain, from, to)) = spans.pop() {
        let Some(&(resolution, width)) = GRAINS.get(grain) else {
            continue;
        };
        let Some(seconds) = width else {
            for (_, max) in series.points(call, resolution, None, from, to).await? {
                peak = peak.max(max);
            }
            continue;
        };
        let whole = ceil(from, seconds)
            .zip(floor(to, seconds))
            .filter(|(start, end)| start < end);
        let Some((start, end)) = whole else {
            spans.push((grain + 1, from, to));
            continue;
        };
        let mut covered = None;
        for (at, max) in series
            .points(call, resolution, Some(seconds), start, end)
            .await?
        {
            peak = peak.max(max);
            let until = at
                .checked_add_signed(TimeDelta::seconds(seconds))
                .unwrap_or(end);
            covered = covered.max(Some(until));
        }
        let Some(covered) = covered else {
            spans.push((grain + 1, from, to));
            continue;
        };
        if from < start {
            spans.push((grain + 1, from, start));
        }
        if covered < to {
            spans.push((grain + 1, covered.max(start), to));
        }
    }
    Ok(peak.round() as u64)
}

struct Series<'a> {
    key: &'a str,
    subject: Id,
}

impl Series<'_> {
    async fn points<P>(
        &self,
        call: &Call<P>,
        resolution: &str,
        width: Option<i64>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<(DateTime<Utc>, f64)>> {
        let mut found = Vec::new();
        let mut windows = vec![(from, to)];
        while let Some((from, to)) = windows.pop() {
            let answer = call
                .invoke(
                    SERIES,
                    json!({
                        "key": self.key,
                        "subject": self.subject,
                        "from": from,
                        "to": to,
                        "resolution": resolution,
                    }),
                )
                .await;
            let series = match answer {
                Err(error) if too_many(&error) => {
                    let middle =
                        halve(from, to, width).ok_or_else(|| unexpected(SERIES, "points"))?;
                    windows.push((middle, to));
                    windows.push((from, middle));
                    continue;
                }
                answer => answer?,
            };
            let points = series["points"]
                .as_array()
                .ok_or_else(|| unexpected(SERIES, "points"))?;
            for point in points {
                let at = point["at"]
                    .as_str()
                    .and_then(|at| at.parse::<DateTime<Utc>>().ok())
                    .ok_or_else(|| unexpected(SERIES, "at"))?;
                let max = point["max"]
                    .as_f64()
                    .ok_or_else(|| unexpected(SERIES, "max"))?;
                found.push((at, max));
            }
        }
        Ok(found)
    }
}

fn too_many(error: &KeelError) -> bool {
    error.key() == SERIES_POINTS
        || error
            .to_call_error()
            .fields
            .iter()
            .any(|field| field.key.as_deref() == Some(SERIES_POINTS))
}

fn halve(from: DateTime<Utc>, to: DateTime<Utc>, width: Option<i64>) -> Option<DateTime<Utc>> {
    let middle = from.checked_add_signed((to - from) / 2)?;
    let middle = match width {
        Some(seconds) => floor(middle, seconds)?,
        None => middle,
    };
    (from < middle && middle < to).then_some(middle)
}

fn floor(at: DateTime<Utc>, seconds: i64) -> Option<DateTime<Utc>> {
    let timestamp = at.timestamp();
    DateTime::from_timestamp(timestamp - timestamp.rem_euclid(seconds), 0)
}

fn ceil(at: DateTime<Utc>, seconds: i64) -> Option<DateTime<Utc>> {
    let floored = floor(at, seconds)?;
    if floored == at {
        return Some(floored);
    }
    floored.checked_add_signed(TimeDelta::seconds(seconds))
}

fn unexpected(function: &str, member: &str) -> KeelError {
    KeelError::internal(ERROR_ANSWER)
        .param("function", function)
        .param("member", member)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use keel_types::{ErrorCode, Level, Principal};

    use super::*;
    use crate::tests::principal;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Quota {
        limit: u32,
    }

    #[derive(Serialize, JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct Usage {
        boxes_used: u32,
    }

    #[derive(Default)]
    struct Boxes {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Boxes {
        fn note(&self, entry: String) {
            self.seen.lock().unwrap().push(entry);
        }
    }

    #[async_trait]
    impl ServiceKind for Boxes {
        const KIND: &'static str = "probe.boxes";
        const TITLE: &'static str = "probe.service.boxes";
        const SCHEMA_VERSION: u32 = 3;
        type Params = Quota;
        type State = Usage;
        type Account = ServiceAccount;

        async fn open(
            &self,
            call: &Call<()>,
            request: OpenRequest<ServiceAccount, Quota>,
        ) -> Result<Opened<Usage>> {
            self.note(format!(
                "open {} {} {} by {}",
                request.external_id,
                request.account.origin,
                request.params.limit,
                call.principal().login
            ));
            Ok(Opened {
                state: Usage { boxes_used: 0 },
            })
        }

        async fn suspend(&self, _: &Call<()>, service: &Service, reason: Value) -> Result<()> {
            self.note(format!("suspend {} {reason}", service.external_id));
            Ok(())
        }

        async fn resume(&self, _: &Call<()>, service: &Service) -> Result<()> {
            self.note(format!("resume {}", service.external_id));
            Ok(())
        }

        async fn close(&self, _: &Call<()>, service: &Service) -> Result<()> {
            self.note(format!("close {}", service.external_id));
            Ok(())
        }

        async fn set_params(
            &self,
            _: &Call<()>,
            service: &Service,
            params: Quota,
        ) -> Result<Usage> {
            self.note(format!("set {} {}", service.external_id, params.limit));
            Ok(Usage {
                boxes_used: params.limit / 2,
            })
        }

        async fn stat(&self, _: &Call<()>, _: &Service, period: Period) -> Result<Value> {
            Ok(json!({ "hours": (period.to - period.from).num_hours() }))
        }

        async fn login(&self, _: &Call<()>, service: &Service) -> Result<LoginTarget> {
            Ok(LoginTarget {
                url: format!("/f/probe.box.list?service={}", service.id),
            })
        }
    }

    fn service() -> Service {
        Service {
            id: "01J9S00000000000000000000S".parse().unwrap(),
            kind: Boxes::KIND.into(),
            external_id: "bill-svc-1".into(),
            account_id: "01J8A0000000000000000000A1".parse().unwrap(),
            status: ServiceStatus::Active,
            params: json!({ "limit": 4 }),
            state: json!({ "boxesUsed": 0 }),
            schema_version: 3,
            peer_id: None,
        }
    }

    fn call() -> Call<Value> {
        crate::call::CallBuilder::new(principal(Level::Admin), "req-svc")
            .read(sea_orm::DatabaseConnection::Disconnected)
            .call(json!({}))
    }

    #[test]
    fn a_kind_definition_keeps_its_name_title_version_and_the_schemas_of_its_functions() {
        let def = ServiceKindDef::new(Boxes::default());
        assert_eq!(def.kind(), "probe.boxes");
        assert_eq!(def.title(), "probe.service.boxes");
        assert_eq!(def.schema_version(), 3);
        let schemas = def.schemas();
        assert_eq!(
            schemas.account.as_value()["required"],
            json!(["origin", "externalId"])
        );
        assert_eq!(
            schemas.account.as_value()["properties"]["origin"]["pattern"],
            json!(EXTERNAL_PATTERN)
        );
        assert_eq!(
            schemas.params.as_value()["properties"]["limit"]["type"],
            json!("integer")
        );
        assert_eq!(
            schemas.state.as_value()["properties"]["boxesUsed"]["type"],
            json!("integer")
        );
        assert_eq!(
            schemas.open.as_value()["required"],
            json!(["externalId", "account", "params"])
        );
        assert_eq!(
            schemas.open.as_value()["properties"]["externalId"]["maxLength"],
            json!(256)
        );
        assert_eq!(
            schemas.set_params.as_value()["required"],
            json!(["externalId", "params"])
        );
        assert_eq!(
            schemas.answer.as_value()["required"],
            json!(["serviceId", "accountId", "status", "state"])
        );
        assert!(format!("{def:?}").contains("probe.boxes"));
    }

    #[tokio::test]
    async fn the_definition_hands_typed_values_to_the_kind_and_answers_its_state_as_json() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let def = ServiceKindDef::new(Boxes { seen: seen.clone() });
        let call = call();
        let service = service();
        let opened = def
            .open(
                &call,
                OpenRequest {
                    id: service.id,
                    external_id: "bill-svc-1".into(),
                    account_id: service.account_id,
                    account: json!({ "origin": "bill", "externalId": "acc-7" }),
                    params: json!({ "limit": 4 }),
                },
            )
            .await
            .unwrap();
        assert_eq!(opened.state, json!({ "boxesUsed": 0 }));
        def.suspend(&call, &service, json!("unpaid")).await.unwrap();
        def.resume(&call, &service).await.unwrap();
        let state = def
            .set_params(&call, &service, json!({ "limit": 10 }))
            .await
            .unwrap();
        assert_eq!(state, json!({ "boxesUsed": 5 }));
        def.close(&call, &service).await.unwrap();
        let period = Period {
            from: "2026-10-01T00:00:00Z".parse().unwrap(),
            to: "2026-10-02T00:00:00Z".parse().unwrap(),
        };
        assert_eq!(
            def.stat(&call, &service, period).await.unwrap(),
            json!({ "hours": 24 })
        );
        assert_eq!(
            def.login(&call, &service).await.unwrap().url,
            format!("/f/probe.box.list?service={}", service.id)
        );
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "open bill-svc-1 bill 4 by alice",
                "suspend bill-svc-1 \"unpaid\"",
                "resume bill-svc-1",
                "set bill-svc-1 10",
                "close bill-svc-1",
            ]
        );
    }

    #[tokio::test]
    async fn values_that_do_not_fit_the_kind_answer_validation_on_their_member() {
        let def = ServiceKindDef::new(Boxes::default());
        let call = call();
        let service = service();
        let refused = def
            .open(
                &call,
                OpenRequest {
                    id: service.id,
                    external_id: "bill-svc-1".into(),
                    account_id: service.account_id,
                    account: json!({ "origin": "bill" }),
                    params: json!({ "limit": 4 }),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Validation);
        assert_eq!(refused.field_path(), Some("account"));
        let refused = def
            .set_params(&call, &service, json!({ "limit": "many" }))
            .await
            .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Validation);
        assert_eq!(refused.field_path(), Some("params"));
    }

    #[test]
    fn a_status_reads_back_from_its_name_and_the_wire_shapes_use_camel_case() {
        for status in ServiceStatus::ALL {
            assert_eq!(ServiceStatus::parse(status.as_str()), Some(status));
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        assert_eq!(ServiceStatus::parse("deleted"), None);
        let answer = ServiceAnswer {
            service_id: service().id,
            account_id: service().account_id,
            status: ServiceStatus::Closed,
            state: json!({}),
            retain_until: None,
        };
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(wire["status"], json!("closed"));
        assert!(wire.get("retainUntil").is_none());
        let suspend: SuspendParams =
            serde_json::from_value(json!({ "externalId": "bill-svc-1" })).unwrap();
        assert_eq!(suspend.reason, Value::Null);
        let account: ServiceAccount =
            serde_json::from_value(json!({ "origin": "bill", "externalId": "acc-7" })).unwrap();
        assert_eq!(account.kind, None);
    }

    struct Accounts;

    #[async_trait]
    impl crate::system::AccountSource for Accounts {
        async fn ancestry(&self, account: Id, _: &crate::db::Db) -> Result<Vec<Id>> {
            Ok(vec![account])
        }

        async fn by_id(
            &self,
            account: Id,
            _: &crate::db::Db,
        ) -> Result<Option<keel_types::Account>> {
            Ok(Some(keel_types::Account {
                id: account,
                kind: "user".into(),
                parent_id: Some(Id::ROOT_ACCOUNT),
                path: format!("/{account}"),
                state: keel_types::AccountState::Active,
                external_id: Some("acc-7".into()),
                origin: Some("bill".into()),
            }))
        }

        async fn of_external_id(
            &self,
            _: &str,
            _: &crate::db::Db,
        ) -> Result<Vec<keel_types::Account>> {
            Ok(Vec::new())
        }
    }

    struct Members(Vec<Principal>);

    #[async_trait]
    impl crate::system::PrincipalSource for Members {
        async fn by_id(&self, _: Id, _: &crate::db::Db) -> Result<Option<Principal>> {
            Ok(None)
        }

        async fn by_login(&self, _: &str, _: &crate::db::Db) -> Result<Option<Principal>> {
            Ok(None)
        }

        async fn by_external(
            &self,
            _: &str,
            _: &str,
            _: &crate::db::Db,
        ) -> Result<Option<Principal>> {
            Ok(None)
        }

        async fn of_external_id(&self, _: &str, _: &crate::db::Db) -> Result<Vec<Principal>> {
            Ok(Vec::new())
        }

        async fn at_or_above(&self, _: Level, _: &crate::db::Db) -> Result<Vec<Principal>> {
            Ok(Vec::new())
        }

        async fn of_account(&self, _: Id, _: &crate::db::Db) -> Result<Vec<Principal>> {
            Ok(self.0.clone())
        }

        async fn touch_acl(&self, _: Id, _: &crate::db::Db) -> Result<()> {
            Ok(())
        }

        async fn create(
            &self,
            _: crate::system::NewPrincipal,
            _: DateTime<Utc>,
            _: &crate::db::Db,
        ) -> Result<Principal> {
            Err(KeelError::internal("keel.error.internal"))
        }
    }

    struct Answers {
        seen: Mutex<Vec<(String, Value)>>,
        answers: Mutex<Vec<Value>>,
    }

    impl crate::call::Invoker for Answers {
        fn invoke(
            &self,
            _: crate::call::CallHandle,
            func: &str,
            params: Value,
        ) -> futures::future::BoxFuture<'static, Result<Value>> {
            self.seen.lock().unwrap().push((func.to_owned(), params));
            let answer = self.answers.lock().unwrap().remove(0);
            Box::pin(async move { Ok(answer) })
        }
    }

    fn member(login: &str, origin: Option<&str>) -> Principal {
        let mut principal = principal(Level::User);
        principal.id = Id::generate();
        principal.login = login.into();
        principal.origin = origin.map(str::to_owned);
        principal
    }

    fn calling(members: Vec<Principal>, answers: Vec<Value>) -> (Call<()>, Arc<Answers>) {
        let invoker = Arc::new(Answers {
            seen: Mutex::new(Vec::new()),
            answers: Mutex::new(answers),
        });
        (through(members, invoker.clone()), invoker)
    }

    fn through(members: Vec<Principal>, invoker: Arc<dyn crate::call::Invoker>) -> Call<()> {
        let mut services = crate::services::Services::new();
        services.provide::<dyn crate::system::AccountSource>(Arc::new(Accounts));
        services.provide::<dyn crate::system::PrincipalSource>(Arc::new(Members(members)));
        crate::call::CallBuilder::new(principal(Level::Admin), "req-svc")
            .services(Arc::new(services))
            .invoker(invoker)
            .read(sea_orm::DatabaseConnection::Disconnected)
            .call(())
    }

    #[tokio::test]
    async fn login_issues_a_short_key_for_the_oldest_principal_carrying_the_origin_of_the_account()
    {
        let by_hand = member("by-hand", None);
        let other = member("other", Some("shop"));
        let customer = member("client-42", Some("bill"));
        let later = member("client-43", Some("bill"));
        let (call, invoker) = calling(
            vec![by_hand, other, customer.clone(), later],
            vec![json!({ "key": "kok_1", "url": "/login?key=kok_1" })],
        );
        let target = login(&call, &service(), "/f/probe.box.list").await.unwrap();
        assert_eq!(target.url, "/login?key=kok_1");
        assert_eq!(
            *invoker.seen.lock().unwrap(),
            [(
                "auth.key.issue".to_owned(),
                json!({ "principal": customer.id, "ttl": LOGIN_TTL_SECONDS, "redirect": "/f/probe.box.list" })
            )]
        );

        let (call, invoker) = calling(vec![member("by-hand", None)], Vec::new());
        let refused = login(&call, &service(), "/").await.unwrap_err();
        assert_eq!(refused.code(), ErrorCode::Conflict);
        assert_eq!(refused.key(), ERROR_NO_PRINCIPAL);
        assert_eq!(
            refused.params()["accountId"],
            json!(service().account_id.to_string())
        );
        assert!(invoker.seen.lock().unwrap().is_empty());

        let (call, _) = calling(vec![member("client-42", Some("bill"))], vec![json!({})]);
        let broken = login(&call, &service(), "/").await.unwrap_err();
        assert_eq!(broken.code(), ErrorCode::Internal);
        assert_eq!(broken.key(), ERROR_ANSWER);
        assert_eq!(broken.params()["function"], json!("auth.key.issue"));
        assert_eq!(broken.params()["member"], json!("url"));
    }

    struct Stored {
        samples: Vec<(DateTime<Utc>, f64)>,
        hours_until: DateTime<Utc>,
        days_until: DateTime<Utc>,
        limit: usize,
        seen: Mutex<Vec<Value>>,
    }

    impl Stored {
        fn new(samples: &[(&str, f64)]) -> Self {
            Self {
                samples: samples
                    .iter()
                    .map(|(at, value)| (instant(at), *value))
                    .collect(),
                hours_until: DateTime::UNIX_EPOCH,
                days_until: DateTime::UNIX_EPOCH,
                limit: 10_000,
                seen: Mutex::new(Vec::new()),
            }
        }

        fn aggregated(mut self, hours_until: &str, days_until: &str) -> Self {
            self.hours_until = instant(hours_until);
            self.days_until = instant(days_until);
            self
        }

        fn answer(&self, params: &Value) -> Result<Value> {
            let from = instant(params["from"].as_str().unwrap());
            let to = instant(params["to"].as_str().unwrap());
            assert!(from < to, "{params}");
            let (width, until) = match params["resolution"].as_str().unwrap() {
                "hour" => (3_600, self.hours_until),
                "day" => (86_400, self.days_until),
                _ => (0, DateTime::<Utc>::MAX_UTC),
            };
            let mut buckets = std::collections::BTreeMap::<DateTime<Utc>, f64>::new();
            for (at, value) in &self.samples {
                let bucket = if width == 0 {
                    *at
                } else {
                    floor(*at, width).unwrap()
                };
                if bucket < until && from <= bucket && bucket < to {
                    let max = buckets.entry(bucket).or_insert(*value);
                    *max = max.max(*value);
                }
            }
            if buckets.len() > self.limit {
                return Err(KeelError::validation("stats.error.points")
                    .field("resolution")
                    .param("max", self.limit));
            }
            let points: Vec<Value> = buckets
                .into_iter()
                .map(|(at, max)| json!({ "at": at, "max": max }))
                .collect();
            Ok(json!({ "points": points }))
        }

        fn truth(&self, period: Period) -> u64 {
            self.samples
                .iter()
                .filter(|(at, _)| period.from <= *at && *at < period.to)
                .fold(0.0_f64, |peak, (_, value)| peak.max(*value))
                .round() as u64
        }
    }

    impl crate::call::Invoker for Stored {
        fn invoke(
            &self,
            _: crate::call::CallHandle,
            func: &str,
            params: Value,
        ) -> futures::future::BoxFuture<'static, Result<Value>> {
            assert_eq!(func, "stats.series");
            let answer = self.answer(&params);
            self.seen.lock().unwrap().push(params);
            Box::pin(async move { answer })
        }
    }

    fn instant(text: &str) -> DateTime<Utc> {
        text.parse().unwrap()
    }

    fn period(from: &str, to: &str) -> Period {
        Period {
            from: instant(from),
            to: instant(to),
        }
    }

    async fn measured(stored: Arc<Stored>, period: Period) -> Result<u64> {
        let call = through(Vec::new(), stored);
        peak(&call, "probe.boxes", &service(), period).await
    }

    fn windows(stored: &Stored) -> Vec<(String, DateTime<Utc>, DateTime<Utc>)> {
        stored
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|params| {
                assert_eq!(params["key"], json!("probe.boxes"));
                assert_eq!(params["subject"], json!(service().account_id));
                (
                    params["resolution"].as_str().unwrap().to_owned(),
                    instant(params["from"].as_str().unwrap()),
                    instant(params["to"].as_str().unwrap()),
                )
            })
            .collect()
    }

    const SAMPLES: [(&str, f64); 7] = [
        ("2026-10-01T20:00:00Z", 11.0),
        ("2026-10-01T21:30:00Z", 3.0),
        ("2026-10-15T10:00:00Z", 6.0),
        ("2026-10-31T23:00:00Z", 2.0),
        ("2026-11-01T20:59:00Z", 5.0),
        ("2026-11-01T21:00:00Z", 9.0),
        ("2026-11-01T22:00:00Z", 12.0),
    ];

    #[tokio::test]
    async fn peak_reads_only_buckets_that_lie_wholly_inside_the_period() {
        let local_month = period("2026-10-01T21:00:00Z", "2026-11-01T21:00:00Z");
        let periods = [
            local_month,
            period("2026-10-01T00:00:00Z", "2026-11-02T00:00:00Z"),
            period("2026-10-01T21:00:00Z", "2026-10-01T21:45:00Z"),
            period("2026-11-01T20:30:00Z", "2026-11-01T22:30:00Z"),
            period("2026-10-16T00:00:00Z", "2026-11-01T20:00:00Z"),
            period("2026-09-01T00:00:00Z", "2026-09-02T00:00:00Z"),
        ];
        let states = [
            ("1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
            ("2026-11-03T00:00:00Z", "2026-11-03T00:00:00Z"),
            ("2026-10-20T00:00:00Z", "2026-10-10T00:00:00Z"),
            ("2026-11-01T21:00:00Z", "2026-11-01T00:00:00Z"),
        ];
        for (hours_until, days_until) in states {
            for period in periods {
                let stored = Arc::new(Stored::new(&SAMPLES).aggregated(hours_until, days_until));
                let peak = measured(stored.clone(), period).await.unwrap();
                assert_eq!(
                    peak,
                    stored.truth(period),
                    "{period:?} aggregated until {hours_until} and {days_until}"
                );
                for (resolution, from, to) in windows(&stored) {
                    assert!(
                        period.from <= from && to <= period.to,
                        "{resolution} {from} {to}"
                    );
                    let width = match resolution.as_str() {
                        "day" => 86_400,
                        "hour" => 3_600,
                        _ => continue,
                    };
                    assert_eq!(floor(from, width), Some(from), "{resolution} {from}");
                    assert_eq!(floor(to, width), Some(to), "{resolution} {to}");
                }
            }
        }

        let stored = Arc::new(
            Stored::new(&SAMPLES).aggregated("2026-11-03T00:00:00Z", "2026-11-03T00:00:00Z"),
        );
        assert_eq!(measured(stored.clone(), local_month).await.unwrap(), 6);
        assert_eq!(
            windows(&stored)
                .into_iter()
                .map(|(resolution, from, to)| format!("{resolution} {from} {to}"))
                .collect::<Vec<_>>(),
            [
                "day 2026-10-02 00:00:00 UTC 2026-11-01 00:00:00 UTC",
                "hour 2026-11-01 00:00:00 UTC 2026-11-01 21:00:00 UTC",
                "hour 2026-10-01 21:00:00 UTC 2026-10-02 00:00:00 UTC",
                "raw 2026-10-01 22:00:00 UTC 2026-10-02 00:00:00 UTC",
            ]
        );

        let empty = Arc::new(Stored::new(&[]));
        let month = period("2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z");
        assert_eq!(measured(empty.clone(), month).await.unwrap(), 0);
        assert_eq!(
            windows(&empty),
            [
                ("day".to_owned(), month.from, month.to),
                ("hour".to_owned(), month.from, month.to),
                ("raw".to_owned(), month.from, month.to),
            ]
        );
    }

    #[tokio::test]
    async fn peak_halves_a_window_stats_refuses_for_its_size() {
        let minutes: Vec<(String, f64)> = (0..600)
            .map(|minute| {
                let at = instant("2026-10-01T00:00:00Z") + TimeDelta::minutes(minute);
                (at.to_rfc3339(), if minute == 437 { 42.0 } else { 1.0 })
            })
            .collect();
        let samples: Vec<(&str, f64)> = minutes
            .iter()
            .map(|(at, value)| (at.as_str(), *value))
            .collect();
        let mut stored = Stored::new(&samples);
        stored.limit = 50;
        let stored = Arc::new(stored);
        let long = period("2026-09-01T00:00:00Z", "2026-11-01T00:00:00Z");
        assert_eq!(measured(stored.clone(), long).await.unwrap(), 42);
        let raw = windows(&stored)
            .into_iter()
            .filter(|(resolution, _, _)| resolution == "raw")
            .count();
        assert!(raw > 12 && raw < 100, "{raw} raw windows");

        let mut stuck = Stored::new(&[("2026-10-01T00:00:30Z", 1.0)]);
        stuck.limit = 0;
        let broken = measured(
            Arc::new(stuck),
            period("2026-10-01T00:00:00Z", "2026-10-01T00:01:00Z"),
        )
        .await
        .unwrap_err();
        assert_eq!(broken.key(), ERROR_ANSWER);
        assert_eq!(broken.params()["member"], json!("points"));
    }

    #[tokio::test]
    async fn peak_refuses_a_malformed_answer_of_stats() {
        let month = period("2026-10-01T00:00:00Z", "2026-11-01T00:00:00Z");
        for (answer, member) in [
            (json!({}), "points"),
            (json!({ "points": [{ "max": 3.0 }] }), "at"),
            (
                json!({ "points": [{ "at": "2026-10-02T00:00:00Z" }] }),
                "max",
            ),
        ] {
            let (call, _) = calling(Vec::new(), vec![answer]);
            let broken = peak(&call, "probe.boxes", &service(), month)
                .await
                .unwrap_err();
            assert_eq!(broken.code(), ErrorCode::Internal);
            assert_eq!(broken.key(), ERROR_ANSWER);
            assert_eq!(broken.params()["function"], json!("stats.series"));
            assert_eq!(broken.params()["member"], json!(member));
        }
    }
}
