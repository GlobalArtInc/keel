use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use keel_types::{EXTERNAL_MAX_LEN, EXTERNAL_PATTERN, Id};
use schemars::{JsonSchema, Schema, schema_for};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::Call;
use crate::error::{KeelError, Result};

const VALIDATION: &str = "keel.error.validation";
pub const SUBJECT: &str = "svc.service";

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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use keel_types::{ErrorCode, Level};

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
}
