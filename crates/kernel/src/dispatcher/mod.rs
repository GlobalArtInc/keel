pub(crate) mod guards;
mod idempotency;
mod list;
mod outbox;
mod schema;
mod strip;
mod validate;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use keel_sdk::runtime::{AfterCommitHook, Before, CallBuilder, CallHandle, Invoker, Stored};
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink, Client, SessionChange};
use keel_sdk::{AfterContext, KeelError};
use keel_types::{CallError, Envelope, ErrorCode, Id, Level, Meta, MetaPrincipal, Mode, Principal};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde_json::{Map, Value};

use crate::clock::{Clock, SystemClock};
use crate::registry::{Function, Registry};

pub use strip::MASK;
pub use validate::SchemaError;

pub const HOOK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Dispatcher {
    inner: Arc<Inner>,
}

struct Inner {
    registry: Arc<Registry>,
    db: DatabaseConnection,
    prepared: BTreeMap<String, Prepared>,
    audit: Option<Arc<dyn AuditSink>>,
    clock: Arc<dyn Clock>,
    hook_timeout: Duration,
}

struct Prepared {
    validator: validate::ParamsValidator,
    params: strip::Plan,
    result: strip::Plan,
    patched: Vec<String>,
}

struct Target<'a> {
    function: &'a Function,
    prepared: &'a Prepared,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallOptions {
    pub request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub client: Option<Client>,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub envelope: Envelope<Value>,
    pub replay: bool,
    pub session: Option<SessionChange>,
}

impl Reply {
    pub fn status(&self) -> u16 {
        self.envelope.status()
    }

    pub fn data(&self) -> Option<&Value> {
        match &self.envelope {
            Envelope::Ok { data, .. } => Some(data),
            Envelope::Err { .. } => None,
        }
    }

    pub fn error(&self) -> Option<&CallError> {
        match &self.envelope {
            Envelope::Ok { .. } => None,
            Envelope::Err { error } => Some(error),
        }
    }
}

struct Executed {
    data: Value,
    replay: bool,
    after: Vec<AfterCommitHook>,
    context: Option<AfterContext>,
    session: Option<SessionChange>,
}

type AfterHooks = Arc<Mutex<Vec<AfterCommitHook>>>;

struct LocalInvoker {
    inner: Arc<Inner>,
    after: AfterHooks,
}

enum Failure {
    Error(KeelError),
    Race { key: String, hash: String },
}

impl From<KeelError> for Failure {
    fn from(error: KeelError) -> Self {
        Failure::Error(error)
    }
}

impl Dispatcher {
    pub fn new(registry: Arc<Registry>, db: DatabaseConnection) -> Result<Self, SchemaError> {
        let mut prepared = BTreeMap::new();
        for function in registry.functions() {
            let descriptor = function.descriptor();
            prepared.insert(
                function.name().to_owned(),
                Prepared {
                    validator: validate::ParamsValidator::new(descriptor)?,
                    params: strip::Plan::of_params(descriptor),
                    result: strip::Plan::of_result(descriptor),
                    patched: function
                        .patched()
                        .iter()
                        .map(|field| field.name.clone())
                        .collect(),
                },
            );
        }
        let audit = registry.services().get::<dyn AuditSink>();
        let clock = registry
            .services()
            .get::<dyn Clock>()
            .unwrap_or_else(|| Arc::new(SystemClock));
        Ok(Self {
            inner: Arc::new(Inner {
                registry,
                db,
                prepared,
                audit,
                clock,
                hook_timeout: HOOK_TIMEOUT,
            }),
        })
    }

    pub fn hook_timeout(mut self, timeout: Duration) -> Self {
        match Arc::get_mut(&mut self.inner) {
            Some(inner) => inner.hook_timeout = timeout,
            None => tracing::warn!("the hook timeout cannot change once the dispatcher is shared"),
        }
        self
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.inner.registry
    }

    pub(crate) fn db(&self) -> &DatabaseConnection {
        &self.inner.db
    }

    pub async fn call(
        &self,
        principal: Principal,
        func: &str,
        params: Value,
        options: CallOptions,
    ) -> Reply {
        let request_id = options.request_id.unwrap_or_else(generate_request_id);
        let started = Instant::now();
        let meta_principal = match meta_principal(&principal) {
            Ok(meta_principal) => meta_principal,
            Err(error) => return error_reply(error, &request_id),
        };
        let Some(target) = self.inner.resolve(func) else {
            return error_reply(
                KeelError::not_found("keel.error.function_not_found").param("function", func),
                &request_id,
            );
        };
        let mode = target.function.def().mode();
        let recorded = (mode == Mode::Write && self.inner.audit.is_some()).then(|| params.clone());
        let outcome = self
            .inner
            .execute(
                &target,
                &principal,
                &request_id,
                params,
                options.idempotency_key.as_deref(),
                options.client,
            )
            .await;
        if let (Some(sink), Some(mut params)) = (&self.inner.audit, recorded)
            && !outcome.as_ref().is_ok_and(|executed| executed.replay)
        {
            target.prepared.params.mask(&mut params);
            sink.record(AuditEvent {
                request_id: request_id.clone(),
                function: target.function.name().to_owned(),
                module: target.function.module().to_owned(),
                mode,
                principal: principal.clone(),
                params,
                outcome: match &outcome {
                    Ok(_) => AuditOutcome::Committed,
                    Err(error) => AuditOutcome::Failed {
                        error: error.to_call_error(),
                    },
                },
                at: self.inner.clock.now(),
                elapsed: started.elapsed(),
            });
        }
        match outcome {
            Ok(executed) => {
                if let Some(context) = executed.context
                    && !executed.after.is_empty()
                {
                    spawn_after_commit(executed.after, context);
                }
                let meta = Meta {
                    request_id,
                    principal: meta_principal,
                };
                Reply {
                    envelope: Envelope::ok(executed.data, meta),
                    replay: executed.replay,
                    session: executed.session,
                }
            }
            Err(error) => error_reply(error, &request_id),
        }
    }
}

pub fn generate_request_id() -> String {
    format!("req-{}", Id::generate().to_string().to_lowercase())
}

pub(crate) fn meta_principal(principal: &Principal) -> Result<Option<MetaPrincipal>, KeelError> {
    if principal.level == Level::Guest {
        return Ok(None);
    }
    principal.meta().map(Some).map_err(|error| {
        let via = serde_json::to_value(&error.0).unwrap_or_default();
        KeelError::internal("keel.error.actor_not_delegated")
            .param("via", via)
            .source(error)
    })
}

fn error_reply(error: KeelError, request_id: &str) -> Reply {
    if error.code() == ErrorCode::Internal {
        tracing::error!(
            request_id,
            error = %error,
            source = ?std::error::Error::source(&error),
            "call failed"
        );
    }
    let mut wire = error.to_call_error();
    wire.request_id = Some(request_id.to_owned());
    Reply {
        envelope: Envelope::err(wire),
        replay: false,
        session: None,
    }
}

fn spawn_after_commit(hooks: Vec<AfterCommitHook>, context: AfterContext) {
    tokio::spawn(async move {
        for hook in hooks {
            if let Err(error) = hook(context.clone()).await {
                tracing::error!(
                    request_id = context.request_id(),
                    error = %error,
                    "after_commit hook failed"
                );
            }
        }
    });
}

impl Inner {
    fn resolve(&self, func: &str) -> Option<Target<'_>> {
        let function = self.registry.function(func)?;
        let prepared = self.prepared.get(func)?;
        Some(Target { function, prepared })
    }

    fn invoker(self: &Arc<Self>, after: AfterHooks) -> Arc<dyn Invoker> {
        match self.registry.invoker() {
            Some(invoker) => invoker.clone(),
            None => Arc::new(LocalInvoker {
                inner: self.clone(),
                after,
            }),
        }
    }

    async fn execute(
        self: &Arc<Self>,
        target: &Target<'_>,
        principal: &Principal,
        request_id: &str,
        params: Value,
        key: Option<&str>,
        client: Option<Client>,
    ) -> Result<Executed, KeelError> {
        let def = target.function.def();
        let after = AfterHooks::default();
        let pool = CallBuilder::new(principal.clone(), request_id)
            .services(self.registry.services().clone())
            .invoker(self.invoker(after.clone()))
            .client(client.clone())
            .read(self.db.clone());
        self.admit(target, &pool, &params, true).await?;
        let mut idempotency = None;
        if def.mode() == Mode::Write
            && principal.level != Level::Guest
            && let Some(key) = key
        {
            let hash = idempotency::hash(&params);
            if let Some(record) = idempotency::lookup(&self.db, principal.id, key).await? {
                return replay(record, def.name(), &hash);
            }
            idempotency = Some((key.to_owned(), hash));
        }
        let (params, extra) = split_extra(params, &target.prepared.patched);
        let body = async {
            let params = self.before(target.function, &pool, params).await?;
            match def.mode() {
                Mode::Read => {
                    let data = self.perform(target, &pool, params).await?;
                    Ok(Executed {
                        data,
                        replay: false,
                        after: Vec::new(),
                        context: None,
                        session: pool.take_session_change(),
                    })
                }
                Mode::Write => {
                    let handle = CallBuilder::new(principal.clone(), request_id)
                        .services(self.registry.services().clone())
                        .invoker(self.invoker(after.clone()))
                        .client(client)
                        .extra(extra)
                        .write(self.db.begin().await?);
                    self.commit(target, handle, params, idempotency, &after)
                        .await
                }
            }
        };
        match def.limits().timeout {
            Some(limit) => tokio::time::timeout(limit, body)
                .await
                .map_err(|_| KeelError::unavailable("keel.error.timeout"))?,
            None => body.await,
        }
    }

    async fn commit(
        self: &Arc<Self>,
        target: &Target<'_>,
        handle: CallHandle,
        params: Value,
        idempotency: Option<(String, String)>,
        after: &AfterHooks,
    ) -> Result<Executed, KeelError> {
        let principal = handle.principal().clone();
        let performed = async {
            let data = self.perform(target, &handle, params).await?;
            if let Some((key, hash)) = idempotency {
                let mut stored = data.clone();
                target.prepared.result.mask_once(&mut stored);
                if let idempotency::Recorded::Taken = idempotency::record(
                    handle.db(),
                    principal.id,
                    &key,
                    target.function.name(),
                    &hash,
                    &stored,
                    self.clock.now(),
                )
                .await?
                {
                    return Err(Failure::Race { key, hash });
                }
            }
            Ok::<Value, Failure>(data)
        }
        .await;
        let outcome = match performed {
            Ok(data) => {
                let mut hooks = take(after);
                hooks.extend(handle.take_after_commit());
                let context = handle.after_context();
                let session = handle.take_session_change();
                let transaction = handle
                    .finish()
                    .map_err(|shared| {
                        KeelError::internal("keel.error.call_still_shared")
                            .param("holders", shared.holders)
                    })?
                    .ok_or_else(|| {
                        KeelError::internal("keel.error.write_call_without_transaction")
                    })?;
                transaction.commit().await?;
                Ok(Executed {
                    data,
                    replay: false,
                    after: hooks,
                    context: Some(context),
                    session,
                })
            }
            Err(failure) => {
                if let Ok(Some(transaction)) = handle.finish() {
                    transaction.rollback().await?;
                }
                match failure {
                    Failure::Error(error) => Err(error),
                    Failure::Race { key, hash } => {
                        match idempotency::lookup(&self.db, principal.id, &key).await? {
                            Some(record) => replay(record, target.function.name(), &hash),
                            None => Err(KeelError::conflict("keel.error.idempotency_race")),
                        }
                    }
                }
            }
        };
        take(after);
        outcome
    }

    async fn admit(
        &self,
        target: &Target<'_>,
        handle: &CallHandle,
        params: &Value,
        entered: bool,
    ) -> Result<(), KeelError> {
        let function = target.function;
        let access = &function.descriptor().access;
        guards::level(access, handle.principal())?;
        guards::permission(&self.registry, access, handle.principal())?;
        if entered {
            guards::scope(function.name(), function.def().mode(), handle.principal())?;
            if !self.registry.guards().is_empty() {
                let probe = handle.call(params.clone());
                for guard in self.registry.guards() {
                    guard.hook.check(function.name(), &probe).await?;
                }
            }
        }
        guards::owned(handle.db(), handle.account_scope(), function.def(), params).await?;
        target
            .prepared
            .params
            .forbidden(handle.principal().level, params)?;
        target.prepared.validator.validate(params)
    }

    async fn before(
        &self,
        function: &Function,
        handle: &CallHandle,
        mut params: Value,
    ) -> Result<Value, KeelError> {
        for hook in function.before() {
            let call = handle.call(params.clone());
            let verdict = tokio::time::timeout(self.hook_timeout, (hook.hook)(call))
                .await
                .map_err(|_| {
                    KeelError::unavailable("keel.error.hook_timeout")
                        .param("module", hook.module.as_str())
                        .param("function", function.name())
                })??;
            if let Before::Enrich(enrichment) = verdict {
                params = enrich(params, enrichment).ok_or_else(|| {
                    KeelError::internal("keel.error.enrich_not_object")
                        .param("module", hook.module.as_str())
                        .param("function", function.name())
                })?;
            }
        }
        Ok(params)
    }

    async fn perform(
        &self,
        target: &Target<'_>,
        handle: &CallHandle,
        params: Value,
    ) -> Result<Value, KeelError> {
        let function = target.function;
        let write = function.def().mode() == Mode::Write;
        if write {
            for hook in function.within() {
                (hook.hook)(handle.call(params.clone())).await?;
            }
        }
        let mut data = (function.def().handler())(handle.call(params)).await?;
        if write {
            for patched in function.patched() {
                let Some(value) = handle.extra().get(&patched.name).cloned() else {
                    continue;
                };
                let stored = Stored {
                    field: patched.name.clone(),
                    value,
                    row: data.clone(),
                };
                (patched.store)(handle.call(Value::Null), stored).await?;
            }
            let events = handle.take_events();
            if !events.is_empty() {
                outbox::write(
                    handle.db(),
                    function.module(),
                    handle.principal(),
                    events,
                    self.clock.now(),
                )
                .await?;
            }
        }
        self.project(target, handle.principal().level, &mut data);
        Ok(data)
    }

    fn project(&self, target: &Target<'_>, level: Level, data: &mut Value) {
        target.prepared.result.strip(level, data);
        target.prepared.result.mask(data);
        if let Some(spec) = target.function.def().list_spec() {
            let result = &target.prepared.result;
            list::totals(spec, |key| result.row_visible(key, level), data);
        }
    }

    fn nested(self: &Arc<Self>, outer: &CallHandle, after: &AfterHooks) -> CallBuilder {
        CallBuilder::new(outer.principal().clone(), outer.request_id())
            .scope(*outer.account_scope())
            .services(self.registry.services().clone())
            .client(outer.client().cloned())
            .invoker(Arc::new(LocalInvoker {
                inner: self.clone(),
                after: after.clone(),
            }))
    }

    async fn invoke_nested(
        self: &Arc<Self>,
        outer: CallHandle,
        after: AfterHooks,
        func: String,
        params: Value,
    ) -> Result<Value, KeelError> {
        let Some(target) = self.resolve(&func) else {
            return Err(
                KeelError::not_found("keel.error.function_not_found").param("function", func)
            );
        };
        let mode = target.function.def().mode();
        if mode == Mode::Write && !outer.db().is_transaction() {
            return Err(
                KeelError::internal("keel.error.invoke_write_outside_transaction")
                    .param("function", func),
            );
        }
        self.admit(&target, &outer, &params, false).await?;
        let (params, extra) = split_extra(params, &target.prepared.patched);
        let pool = self.nested(&outer, &after).read(self.db.clone());
        let params = self.before(target.function, &pool, params).await?;
        let handle = self
            .nested(&outer, &after)
            .extra(extra)
            .with_db(mode, outer.db().clone());
        drop(outer);
        let data = self.perform(&target, &handle, params).await?;
        after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(handle.take_after_commit());
        Ok(data)
    }
}

fn take(after: &AfterHooks) -> Vec<AfterCommitHook> {
    std::mem::take(&mut *after.lock().unwrap_or_else(PoisonError::into_inner))
}

fn replay(record: idempotency::Record, func: &str, hash: &str) -> Result<Executed, KeelError> {
    if record.func == func && record.request_hash == hash {
        Ok(Executed {
            data: record.response,
            replay: true,
            after: Vec::new(),
            context: None,
            session: None,
        })
    } else {
        Err(KeelError::idempotency_mismatch(
            "keel.error.idempotency_mismatch",
        ))
    }
}

fn split_extra(params: Value, patched: &[String]) -> (Value, Map<String, Value>) {
    let mut extra = Map::new();
    if patched.is_empty() {
        return (params, extra);
    }
    let mut params = params;
    if let Value::Object(object) = &mut params {
        for name in patched {
            if let Some(value) = object.remove(name) {
                extra.insert(name.clone(), value);
            }
        }
    }
    (params, extra)
}

fn enrich(params: Value, enrichment: Value) -> Option<Value> {
    match (params, enrichment) {
        (Value::Object(mut base), Value::Object(added)) => {
            base.extend(added);
            Some(Value::Object(base))
        }
        _ => None,
    }
}

impl Invoker for LocalInvoker {
    fn invoke(
        &self,
        call: CallHandle,
        func: &str,
        params: Value,
    ) -> BoxFuture<'static, Result<Value, KeelError>> {
        let inner = self.inner.clone();
        let after = self.after.clone();
        let func = func.to_owned();
        Box::pin(async move { inner.invoke_nested(call, after, func, params).await })
    }
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("functions", &self.inner.prepared.keys().collect::<Vec<_>>())
            .field("audit", &self.inner.audit.is_some())
            .field("hook_timeout", &self.inner.hook_timeout)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn enrichment_merges_an_object_key_by_key_and_refuses_anything_else() {
        assert_eq!(
            enrich(json!({ "a": 1, "b": 2 }), json!({ "b": 3, "c": 4 })),
            Some(json!({ "a": 1, "b": 3, "c": 4 }))
        );
        assert_eq!(enrich(json!({ "a": 1 }), json!([1])), None);
        assert_eq!(enrich(json!({ "a": 1 }), json!("b")), None);
        assert_eq!(enrich(json!({ "a": 1 }), json!(null)), None);
        assert_eq!(enrich(json!(null), json!({ "a": 1 })), None);
    }

    #[test]
    fn patched_fields_move_from_the_parameters_into_extra() {
        let (params, extra) = split_extra(
            json!({ "title": "x", "acmeSync": true }),
            &["acmeSync".to_owned()],
        );
        assert_eq!(params, json!({ "title": "x" }));
        assert_eq!(extra.get("acmeSync"), Some(&json!(true)));
        let (params, extra) = split_extra(json!({ "title": "x" }), &[]);
        assert_eq!(params, json!({ "title": "x" }));
        assert!(extra.is_empty());
    }

    #[test]
    fn a_replay_needs_the_same_function_and_the_same_body() {
        let record = || idempotency::Record {
            func: "t.item.edit".into(),
            request_hash: "abc".into(),
            response: json!({ "id": 1 }),
        };
        let executed = replay(record(), "t.item.edit", "abc").unwrap();
        assert!(executed.replay);
        assert_eq!(executed.data, json!({ "id": 1 }));
        for (func, hash) in [("t.item.edit", "xyz"), ("t.item.delete", "abc")] {
            let Err(error) = replay(record(), func, hash) else {
                panic!("{func} with body {hash} must not replay");
            };
            assert_eq!(error.code(), ErrorCode::IdempotencyMismatch);
        }
    }
}
