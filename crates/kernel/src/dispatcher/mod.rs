mod caller;
mod group;
pub(crate) mod guards;
mod idempotency;
mod list;
mod live;
mod masks;
mod outbox;
mod raw;
mod schema;
pub(crate) mod strip;
mod task;
mod validate;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use futures::future::BoxFuture;
use keel_sdk::runtime::{
    AfterCommitHook, Before, CallBuilder, CallHandle, Invoked, Invoker, Stored,
};
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink, Client, SessionChange, TaskRunner};
use keel_sdk::{AfterContext, Download, FunctionDef, KeelError, TaskHandle};
use keel_types::{
    CallError, Envelope, ErrorCode, EventSubject, GroupResult, Id, Kind, Level, Meta,
    MetaPrincipal, Mode, Principal,
};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde_json::{Map, Value};

use crate::clock::{Clock, SystemClock};
use crate::registry::{Function, Registry};

pub use caller::CallerHandle;
pub use group::{GROUP_BODY_KEY, MULTI_FIELD_KEY, MULTI_UNMASKED_KEY};
pub use live::{LIVE_CAPACITY, LiveBus};
pub(crate) use masks::MaskHandle;
pub use outbox::{MAX_BATCH as OUTBOX_BATCH, OutboxFeed};
pub use raw::{
    DOWNLOAD_ROUTE_KEY, RAW_PREFIX, UPLOAD_ROUTE_KEY, download_route_error, raw_route,
    upload_route_error,
};
pub use strip::MASK;
pub use task::BATCH_LOG_PREFIX;
pub use validate::{FILE_ENCODING_KEY, FILE_TOO_LARGE_KEY, FILE_TYPE_KEY, SchemaError};

pub(crate) use strip::Plan;
pub(crate) use validate::{file_encoding, file_too_large, file_type};

pub const GROUP_READ_KEY: &str = "keel.error.group_read";

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
    runner: Option<Arc<dyn TaskRunner>>,
    outbox: Option<Arc<OutboxFeed>>,
    clock: Arc<dyn Clock>,
    hook_timeout: Duration,
}

struct Prepared {
    validator: validate::ParamsValidator,
    group: Option<validate::ParamsValidator>,
    params: strip::Plan,
    result: strip::Plan,
    patched: Vec<String>,
}

enum Work {
    Single(Value),
    Group(group::Group),
}

struct Target<'a> {
    function: &'a Function,
    prepared: &'a Prepared,
}

#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub request_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub client: Option<Client>,
    pub tz: Option<Tz>,
    pub task: Option<TaskHandle>,
}

#[derive(Debug)]
pub struct Reply {
    pub envelope: Envelope<Value>,
    pub replay: bool,
    pub session: Option<SessionChange>,
    pub download: Option<Download>,
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
    download: Option<Download>,
}

struct Performed {
    data: Value,
    download: Option<Download>,
}

fn audited(def: &FunctionDef) -> bool {
    def.mode() == Mode::Write || matches!(def.kind(), Kind::Download | Kind::Upload)
}

struct Origin {
    client: Option<Client>,
    tz: Tz,
    task: Option<TaskHandle>,
}

struct Recorded {
    params: Value,
    client: Option<Client>,
}

impl Recorded {
    fn event(
        self,
        target: &Target<'_>,
        principal: &Principal,
        request_id: &str,
        outcome: &Result<Executed, KeelError>,
        at: DateTime<Utc>,
        elapsed: Duration,
    ) -> AuditEvent {
        let def = target.function.def();
        let Recorded { mut params, client } = self;
        let subject = def
            .key()
            .and_then(|key| subject_of(&params, key).or_else(|| subject_of(&params, group::IDS)));
        target.prepared.params.mask(&mut params);
        let result = match outcome {
            Ok(executed) if def.kind() == Kind::Form && def.key().is_some() => {
                let mut result = executed.data.clone();
                target.prepared.result.mask_once(&mut result);
                Some(result)
            }
            _ => None,
        };
        AuditEvent {
            request_id: request_id.to_owned(),
            function: def.name().to_owned(),
            module: def.module().to_owned(),
            mode: def.mode(),
            principal: principal.clone(),
            client,
            params,
            subject,
            result,
            outcome: match outcome {
                Ok(_) => AuditOutcome::Committed,
                Err(error) => AuditOutcome::Failed {
                    error: error.to_call_error(),
                },
            },
            at,
            elapsed,
        }
    }
}

fn subject_of(params: &Value, key: &str) -> Option<String> {
    match params.get(key)? {
        Value::Null => None,
        Value::String(id) => Some(id.clone()),
        other => Some(other.to_string()),
    }
}

fn single_id(params: &Value, key: &str) -> Option<Id> {
    params.get(key)?.as_str()?.parse().ok()
}

fn subject_kind(function: &str) -> &str {
    function.rsplit_once('.').map_or(function, |(kind, _)| kind)
}

#[derive(Default)]
struct CallShared {
    after: Mutex<Vec<AfterCommitHook>>,
    outbox: AtomicU64,
}

type Shared = Arc<CallShared>;

impl CallShared {
    fn wrote(&self, seq: u64) {
        self.outbox.fetch_max(seq, Ordering::SeqCst);
    }

    fn written(&self) -> Option<u64> {
        match self.outbox.load(Ordering::SeqCst) {
            0 => None,
            seq => Some(seq),
        }
    }
}

struct LocalInvoker {
    inner: Arc<Inner>,
    after: Shared,
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
            let group = match (
                group::accepts_groups(function.def(), descriptor),
                &descriptor.ui,
            ) {
                (true, Some(ui)) => {
                    Some(validate::ParamsValidator::partial(descriptor, &ui.multi)?)
                }
                _ => None,
            };
            prepared.insert(
                function.name().to_owned(),
                Prepared {
                    validator: validate::ParamsValidator::new(descriptor)?,
                    group,
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
        let runner = registry.services().get::<dyn TaskRunner>();
        let outbox = registry.services().get::<OutboxFeed>();
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
                runner,
                outbox,
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

    pub fn fill_caller(&self) -> bool {
        match self.inner.registry.services().get::<CallerHandle>() {
            Some(handle) => {
                handle.fill(self.clone());
                true
            }
            None => false,
        }
    }

    pub fn fill_outbox(&self, instance: Id) -> bool {
        match &self.inner.outbox {
            Some(feed) => {
                feed.fill(self.inner.db.clone(), instance);
                true
            }
            None => false,
        }
    }

    pub fn db(&self) -> &DatabaseConnection {
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
        let recorded =
            (audited(target.function.def()) && self.inner.audit.is_some()).then(|| Recorded {
                params: params.clone(),
                client: options.client.clone(),
            });
        let outcome = self
            .inner
            .execute(
                &target,
                &principal,
                &request_id,
                params,
                options.idempotency_key.as_deref(),
                Origin {
                    client: options.client,
                    tz: options.tz.unwrap_or(Tz::UTC),
                    task: options.task,
                },
            )
            .await;
        if let (Some(sink), Some(recorded)) = (&self.inner.audit, recorded)
            && !outcome.as_ref().is_ok_and(|executed| executed.replay)
        {
            sink.record(recorded.event(
                &target,
                &principal,
                &request_id,
                &outcome,
                self.inner.clock.now(),
                started.elapsed(),
            ));
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
                    download: executed.download,
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
        download: None,
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

    fn invoker(self: &Arc<Self>, after: Shared) -> Arc<dyn Invoker> {
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
        origin: Origin,
    ) -> Result<Executed, KeelError> {
        let def = target.function.def();
        let after = Shared::default();
        let pool = CallBuilder::new(principal.clone(), request_id)
            .services(self.registry.services().clone())
            .invoker(self.invoker(after.clone()))
            .client(origin.client.clone())
            .tz(origin.tz)
            .task(origin.task.clone())
            .read(self.db.clone());
        let group = group::detect(def, target.function.descriptor(), &params)?;
        match &group {
            Some(group) => self.admit_group(target, &pool, &params, group).await?,
            None => self.admit(target, &pool, &params, true).await?,
        }
        let batch_key = match &group {
            Some(_) => Some(group::IDS),
            None => def.key(),
        };
        let mut idempotency = None;
        if def.mode() == Mode::Write
            && principal.level != Level::Guest
            && let Some(key) = key
        {
            let hash = idempotency::hash(&params);
            if let Some(record) =
                idempotency::lookup(&self.db, principal.id, key, self.clock.now()).await?
            {
                return replay(record, def.name(), &hash);
            }
            idempotency = Some((key.to_owned(), hash));
        }
        let (params, extra) = split_extra(params, &target.prepared.patched);
        if let Some(task) = &origin.task
            && let Some(chunks) = task::chunks(def, batch_key, &params)
        {
            return self
                .chunked(target, &pool, params, chunks, task.clone())
                .await;
        }
        let body = async {
            let params = match &group {
                Some(_) => params,
                None => self.before(target.function, &pool, params).await?,
            };
            if origin.task.is_none()
                && let Some(spawned) = self
                    .entrance(
                        target,
                        &pool,
                        &params,
                        batch_key,
                        &extra,
                        idempotency.as_ref(),
                    )
                    .await?
            {
                return Ok(spawned);
            }
            match def.mode() {
                Mode::Read => {
                    let performed = self.perform(target, &pool, params, &after).await?;
                    Ok(Executed {
                        data: performed.data,
                        replay: false,
                        after: Vec::new(),
                        context: None,
                        session: pool.take_session_change(),
                        download: performed.download,
                    })
                }
                Mode::Write => {
                    let handle = CallBuilder::new(principal.clone(), request_id)
                        .services(self.registry.services().clone())
                        .invoker(self.invoker(after.clone()))
                        .client(origin.client)
                        .tz(origin.tz)
                        .task(origin.task)
                        .extra(extra)
                        .write(self.db.begin().await?);
                    let work = match group {
                        Some(group) => Work::Group(group),
                        None => Work::Single(params),
                    };
                    self.commit(target, handle, work, idempotency, &after).await
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
        work: Work,
        idempotency: Option<(String, String)>,
        after: &Shared,
    ) -> Result<Executed, KeelError> {
        let principal = handle.principal().clone();
        let performed = async {
            let Performed { data, download } = match work {
                Work::Single(params) => self.perform(target, &handle, params, after).await?,
                Work::Group(group) => self.perform_group(target, &handle, group, after).await?,
            };
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
            Ok::<Performed, Failure>(Performed { data, download })
        }
        .await;
        let outcome = match performed {
            Ok(Performed { data, download }) => {
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
                if let (Some(feed), Some(seq)) = (&self.outbox, after.written()) {
                    feed.publish(seq);
                }
                Ok(Executed {
                    data,
                    replay: false,
                    after: hooks,
                    context: Some(context),
                    session,
                    download,
                })
            }
            Err(failure) => {
                if let Ok(Some(transaction)) = handle.finish() {
                    transaction.rollback().await?;
                }
                match failure {
                    Failure::Error(error) => Err(error),
                    Failure::Race { key, hash } => {
                        match idempotency::lookup(&self.db, principal.id, &key, self.clock.now())
                            .await?
                        {
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
        guards::internal(function.def(), handle.principal())?;
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

    async fn admit_group(
        &self,
        target: &Target<'_>,
        handle: &CallHandle,
        params: &Value,
        group: &group::Group,
    ) -> Result<(), KeelError> {
        let function = target.function;
        let def = function.def();
        let descriptor = function.descriptor();
        guards::internal(def, handle.principal())?;
        guards::level(&descriptor.access, handle.principal())?;
        guards::permission(&self.registry, &descriptor.access, handle.principal())?;
        guards::scope(function.name(), def.mode(), handle.principal())?;
        if !self.registry.guards().is_empty() {
            let probe = handle.call(params.clone());
            for guard in self.registry.guards() {
                guard.hook.check(function.name(), &probe).await?;
            }
        }
        let Some(key) = def.key() else {
            return Err(KeelError::internal(GROUP_READ_KEY).param("function", def.name()));
        };
        guards::owned(
            handle.db(),
            handle.account_scope(),
            def,
            &group::keyed(key, &group.ids),
        )
        .await?;
        let multi = descriptor
            .ui
            .as_ref()
            .map(|ui| ui.multi.as_slice())
            .unwrap_or_default();
        group::check_mask(
            group,
            multi,
            &target.prepared.params,
            handle.principal().level,
        )?;
        if let Some(validator) = &target.prepared.group {
            validator.validate_at(group::FIELDS, &Value::Object(group.fields.clone()))?;
        }
        Ok(())
    }

    async fn perform_group(
        self: &Arc<Self>,
        target: &Target<'_>,
        handle: &CallHandle,
        group: group::Group,
        after: &Shared,
    ) -> Result<Performed, KeelError> {
        let def = target.function.def();
        let descriptor = target.function.descriptor();
        let (Some(key), Some(read)) = (
            def.key(),
            descriptor.ui.as_ref().and_then(|ui| ui.read.as_deref()),
        ) else {
            return Err(KeelError::internal(GROUP_READ_KEY).param("function", def.name()));
        };
        let read_key = self
            .registry
            .function(read)
            .and_then(|function| function.def().key())
            .unwrap_or(key)
            .to_owned();
        let properties = group::properties(descriptor);
        let mut updated = 0;
        for id in &group.ids {
            let mut pick = Map::new();
            pick.insert(read_key.clone(), id.clone());
            let current = self
                .invoke_nested(
                    handle.clone(),
                    after.clone(),
                    read.to_owned(),
                    Value::Object(pick),
                )
                .await?;
            let merged = group::merge(
                current,
                &properties,
                &target.prepared.params,
                key,
                id,
                &group.fields,
            );
            self.invoke_nested(handle.clone(), after.clone(), def.name().to_owned(), merged)
                .await?;
            updated += 1;
        }
        Ok(Performed {
            data: serde_json::to_value(GroupResult::new(updated))?,
            download: None,
        })
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
        shared: &Shared,
    ) -> Result<Performed, KeelError> {
        let function = target.function;
        let def = function.def();
        let write = def.mode() == Mode::Write;
        let keyed = write
            .then(|| def.key().and_then(|key| single_id(&params, key)))
            .flatten();
        if write {
            for hook in function.within() {
                (hook.hook)(handle.call(params.clone())).await?;
            }
        }
        handle.set_invoked(Invoked {
            function: def.name().to_owned(),
            params: params.clone(),
            class: def.class().to_owned(),
            resumable: def.is_resumable(),
        });
        let mut data = (def.handler())(handle.call(params)).await?;
        let download = handle.take_download();
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
            let mut events = handle.take_events();
            if let Some(id) = keyed {
                let account = handle
                    .resolved_account()
                    .unwrap_or(handle.principal().account_id);
                let derived = EventSubject {
                    kind: subject_kind(function.name()).to_owned(),
                    id,
                    account_id: Some(account),
                };
                for event in &mut events {
                    event.subject.get_or_insert_with(|| derived.clone());
                }
            }
            if let Some(seq) = outbox::write(
                handle.db(),
                function.module(),
                handle.principal(),
                events,
                self.clock.now(),
            )
            .await?
            {
                shared.wrote(seq);
            }
        }
        self.project(target, handle.principal().level, &mut data);
        Ok(Performed { data, download })
    }

    fn project(&self, target: &Target<'_>, level: Level, data: &mut Value) {
        target.prepared.result.strip(level, data);
        target.prepared.result.mask(data);
        if let Some(spec) = target.function.def().list_spec() {
            let result = &target.prepared.result;
            list::totals(spec, |key| result.row_visible(key, level), data);
        }
    }

    fn nested(self: &Arc<Self>, outer: &CallHandle, after: &Shared) -> CallBuilder {
        CallBuilder::new(outer.principal().clone(), outer.request_id())
            .scope(*outer.account_scope())
            .services(self.registry.services().clone())
            .client(outer.client().cloned())
            .tz(outer.tz())
            .task(outer.task().cloned())
            .invoker(Arc::new(LocalInvoker {
                inner: self.clone(),
                after: after.clone(),
            }))
    }

    async fn invoke_nested(
        self: &Arc<Self>,
        outer: CallHandle,
        after: Shared,
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
        if target.function.def().kind() == Kind::Download {
            return Err(download_route_error(target.function));
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
        let performed = self.perform(&target, &handle, params, &after).await?;
        after
            .after
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(handle.take_after_commit());
        Ok(performed.data)
    }
}

fn take(after: &Shared) -> Vec<AfterCommitHook> {
    std::mem::take(&mut *after.after.lock().unwrap_or_else(PoisonError::into_inner))
}

fn replay(record: idempotency::Record, func: &str, hash: &str) -> Result<Executed, KeelError> {
    if record.func == func && record.request_hash == hash {
        Ok(Executed {
            data: record.response,
            replay: true,
            after: Vec::new(),
            context: None,
            session: None,
            download: None,
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
    fn the_subject_is_the_key_parameter_as_text_and_absent_when_it_is_null_or_missing() {
        assert_eq!(
            subject_of(&json!({ "id": "01J" }), "id"),
            Some("01J".to_owned())
        );
        assert_eq!(
            subject_of(&json!({ "ids": ["a", "b"] }), "ids"),
            Some(r#"["a","b"]"#.to_owned())
        );
        assert_eq!(subject_of(&json!({ "id": null }), "id"), None);
        assert_eq!(subject_of(&json!({ "title": "x" }), "id"), None);
        assert_eq!(subject_of(&json!("bare"), "id"), None);
    }

    #[test]
    fn a_derived_subject_takes_a_single_id_and_the_function_name_without_its_verb() {
        let id: Id = "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().unwrap();
        assert_eq!(single_id(&json!({ "id": id.to_string() }), "id"), Some(id));
        assert_eq!(single_id(&json!({ "ids": [id.to_string()] }), "ids"), None);
        assert_eq!(single_id(&json!({ "id": "nope" }), "id"), None);
        assert_eq!(single_id(&json!({ "id": null }), "id"), None);
        assert_eq!(single_id(&json!({ "title": "x" }), "id"), None);
        assert_eq!(subject_kind("note.edit"), "note");
        assert_eq!(subject_kind("dns.zone.create"), "dns.zone");
        assert_eq!(subject_kind("bare"), "bare");
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
