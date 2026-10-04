use std::any::type_name;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use chrono_tz::Tz;
use futures::future::BoxFuture;
use keel_types::{EventSubject, Id, Mode, Principal, TaskRef};
use sea_orm::{DatabaseConnection, DatabaseTransaction};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::access::AccountScope;
use crate::db::Db;
use crate::download::Download;
use crate::error::{KeelError, Result};
use crate::services::Services;
use crate::settings::SettingsSource;
use crate::stream::Lines;
use crate::system::{Client, LimitSource, SessionChange, TaskRunner, TaskSpec};
use crate::task::TaskHandle;

pub const TASKS_UNAVAILABLE_KEY: &str = "keel.error.tasks_unavailable";
const LIMIT_EXCEEDED_KEY: &str = "keel.error.limit_exceeded";
const SERVICE_MISSING_KEY: &str = "keel.error.service_missing";

pub struct Call<P> {
    state: Arc<CallState>,
    params: P,
}

pub(crate) struct CallState {
    principal: Principal,
    request_id: String,
    mode: Mode,
    scope: AccountScope,
    db: Db,
    extra: Map<String, Value>,
    services: Arc<Services>,
    invoker: Option<Arc<dyn Invoker>>,
    client: Option<Client>,
    tz: Tz,
    task: Option<TaskHandle>,
    events: Mutex<Vec<Emitted>>,
    after_commit: Mutex<Vec<AfterCommitHook>>,
    session: Mutex<Option<SessionChange>>,
    resolved: Mutex<Option<Id>>,
    download: Mutex<Option<Download>>,
    lines: Mutex<Option<Lines>>,
    invoked: Mutex<Option<Invoked>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Invoked {
    pub function: String,
    pub params: Value,
    pub class: String,
    pub resumable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Emitted {
    pub kind: String,
    pub subject: Option<EventSubject>,
    pub data: Value,
}

pub type AfterCommitHook = Box<dyn FnOnce(AfterContext) -> BoxFuture<'static, Result<()>> + Send>;

#[derive(Clone, Debug)]
pub struct AfterContext {
    principal: Principal,
    request_id: String,
    db: Option<Db>,
}

impl AfterContext {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn db(&self) -> Result<&Db> {
        self.db
            .as_ref()
            .ok_or_else(|| KeelError::internal(SERVICE_MISSING_KEY).param("service", "database"))
    }

    #[doc(hidden)]
    pub fn with_db(mut self, db: Db) -> Self {
        self.db = Some(db);
        self
    }
}

pub trait Invoker: Send + Sync {
    fn invoke(
        &self,
        call: CallHandle,
        func: &str,
        params: Value,
    ) -> BoxFuture<'static, Result<Value>>;
}

impl<P> Call<P> {
    pub fn principal(&self) -> &Principal {
        &self.state.principal
    }

    pub fn params(&self) -> &P {
        &self.params
    }

    pub fn request_id(&self) -> &str {
        &self.state.request_id
    }

    pub fn mode(&self) -> Mode {
        self.state.mode
    }

    pub fn account_scope(&self) -> &AccountScope {
        &self.state.scope
    }

    pub fn tz(&self) -> Tz {
        self.state.tz
    }

    pub fn task(&self) -> Option<&TaskHandle> {
        self.state.task.as_ref()
    }

    pub fn db(&self) -> &Db {
        &self.state.db
    }

    pub fn tx(&self) -> Result<&Db> {
        if self.state.db.is_transaction() {
            Ok(&self.state.db)
        } else {
            Err(KeelError::internal(
                "keel.error.read_call_has_no_transaction",
            ))
        }
    }

    pub fn extra(&self) -> &Map<String, Value> {
        &self.state.extra
    }

    pub fn emit(&self, kind: &str, data: &impl Serialize) -> Result<()> {
        self.record_event(kind, None, data)
    }

    pub fn emit_about(
        &self,
        kind: &str,
        subject: EventSubject,
        data: &impl Serialize,
    ) -> Result<()> {
        self.record_event(kind, Some(subject), data)
    }

    fn record_event(
        &self,
        kind: &str,
        subject: Option<EventSubject>,
        data: &impl Serialize,
    ) -> Result<()> {
        if self.state.mode == Mode::Read {
            return Err(KeelError::internal("keel.error.read_call_has_no_outbox"));
        }
        let data = serde_json::to_value(data)?;
        self.state
            .events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Emitted {
                kind: kind.to_owned(),
                subject,
                data,
            });
        Ok(())
    }

    pub fn invoke(&self, func: &str, params: Value) -> BoxFuture<'static, Result<Value>> {
        match &self.state.invoker {
            Some(invoker) => invoker.invoke(CallHandle(self.state.clone()), func, params),
            None => Box::pin(std::future::ready(Err(KeelError::internal(
                "keel.error.invoke_unavailable",
            )))),
        }
    }

    pub fn after_commit<F, Fut>(&self, hook: F)
    where
        F: FnOnce(AfterContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.state
            .after_commit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Box::new(move |context| Box::pin(hook(context))));
    }

    pub fn require<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.state.services.get::<T>().ok_or_else(|| {
            KeelError::internal("keel.error.service_missing")
                .source(std::io::Error::other(type_name::<T>()))
        })
    }

    pub fn config<T: Send + Sync + 'static>(&self) -> Result<Arc<T>> {
        self.state.services.get::<T>().ok_or_else(|| {
            KeelError::internal("keel.error.config_missing")
                .source(std::io::Error::other(type_name::<T>()))
        })
    }

    pub async fn setting<T: DeserializeOwned>(&self, key: &str) -> Result<T> {
        let source = self.require::<dyn SettingsSource>()?;
        let value = source
            .resolve(key, self.state.principal.account_id, &self.state.db)
            .await?;
        serde_json::from_value(value.unwrap_or(Value::Null)).map_err(|error| {
            KeelError::internal("keel.error.setting_value")
                .param("key", key)
                .source(error)
        })
    }

    pub async fn check_limit(&self, key: &str, current: u64) -> Result<()> {
        let account = self
            .state
            .resolved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(self.state.principal.account_id);
        let limits = self.require::<dyn LimitSource>()?;
        match limits.effective(key, account, &self.state.db).await? {
            Some(max) if current >= max => Err(KeelError::limit_exceeded(LIMIT_EXCEEDED_KEY)
                .param("limit", key)
                .param("max", max)),
            _ => Ok(()),
        }
    }

    pub async fn spawn_self_as_task(&self) -> Result<TaskRef> {
        if self.state.task.is_some() {
            return Err(KeelError::internal("keel.error.spawn_inside_task"));
        }
        let runner = self
            .state
            .services
            .get::<dyn TaskRunner>()
            .ok_or_else(|| KeelError::unavailable(TASKS_UNAVAILABLE_KEY))?;
        let invoked = self
            .state
            .invoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| KeelError::internal("keel.error.invocation_unknown"))?;
        let mut params = invoked.params;
        if let Value::Object(object) = &mut params {
            object.extend(self.state.extra.clone());
        }
        let spec = TaskSpec {
            function: invoked.function,
            params,
            principal: self.state.principal.clone(),
            class: invoked.class,
            resumable: invoked.resumable,
            request_id: Some(self.state.request_id.clone()),
        };
        if !self.state.db.is_transaction() {
            return Ok(TaskRef::new(runner.spawn(spec).await?));
        }
        let id = runner.enqueue(&self.state.db, spec).await?;
        self.after_commit(move |_| async move { runner.wake(id).await });
        Ok(TaskRef::new(id))
    }

    pub async fn resolve_account(&self, requested: Option<Id>) -> Result<Id> {
        let own = self.state.principal.account_id;
        let resolved = match requested {
            None => own,
            Some(requested)
                if requested == own
                    || self.state.scope.includes(&self.state.db, requested).await? =>
            {
                requested
            }
            Some(_) => {
                return Err(KeelError::not_found("keel.error.account_not_found").field("account"));
            }
        };
        *self
            .state
            .resolved
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(resolved);
        Ok(resolved)
    }

    pub(crate) fn unit(&self) -> Call<()> {
        Call::from_parts(self.state.clone(), ())
    }

    pub(crate) fn client_slot(&self) -> Option<&Client> {
        self.state.client.as_ref()
    }

    pub(crate) fn record_session(&self, change: SessionChange) {
        *self
            .state
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(change);
    }

    pub(crate) fn state_clone(&self) -> Arc<CallState> {
        self.state.clone()
    }

    pub(crate) fn attach_download(&self, download: Download) {
        *self
            .state
            .download
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(download);
    }

    pub(crate) fn attach_lines(&self, lines: Lines) {
        *self
            .state
            .lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(lines);
    }

    pub(crate) fn into_parts(self) -> (Arc<CallState>, P) {
        (self.state, self.params)
    }

    pub(crate) fn from_parts(state: Arc<CallState>, params: P) -> Self {
        Self { state, params }
    }
}

impl<P: fmt::Debug> fmt::Debug for Call<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("principal", &self.state.principal.login)
            .field("request_id", &self.state.request_id)
            .field("mode", &self.state.mode)
            .field("params", &self.params)
            .finish()
    }
}

pub struct CallBuilder {
    principal: Principal,
    request_id: String,
    scope: Option<AccountScope>,
    extra: Map<String, Value>,
    services: Arc<Services>,
    invoker: Option<Arc<dyn Invoker>>,
    client: Option<Client>,
    tz: Tz,
    task: Option<TaskHandle>,
}

impl CallBuilder {
    pub fn new(principal: Principal, request_id: impl Into<String>) -> Self {
        Self {
            principal,
            request_id: request_id.into(),
            scope: None,
            extra: Map::new(),
            services: Arc::new(Services::new()),
            invoker: None,
            client: None,
            tz: Tz::UTC,
            task: None,
        }
    }

    pub fn client(mut self, client: Option<Client>) -> Self {
        self.client = client;
        self
    }

    pub fn task(mut self, task: Option<TaskHandle>) -> Self {
        self.task = task;
        self
    }

    pub fn tz(mut self, tz: Tz) -> Self {
        self.tz = tz;
        self
    }

    pub fn scope(mut self, scope: AccountScope) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn extra(mut self, extra: Map<String, Value>) -> Self {
        self.extra = extra;
        self
    }

    pub fn services(mut self, services: Arc<Services>) -> Self {
        self.services = services;
        self
    }

    pub fn invoker(mut self, invoker: Arc<dyn Invoker>) -> Self {
        self.invoker = Some(invoker);
        self
    }

    pub fn read(self, connection: DatabaseConnection) -> CallHandle {
        self.build(Mode::Read, Db::from_connection(connection))
    }

    pub fn write(self, transaction: DatabaseTransaction) -> CallHandle {
        self.build(Mode::Write, Db::from_transaction(transaction))
    }

    #[doc(hidden)]
    pub fn with_db(self, mode: Mode, db: Db) -> CallHandle {
        self.build(mode, db)
    }

    fn build(self, mode: Mode, db: Db) -> CallHandle {
        let scope = self
            .scope
            .unwrap_or_else(|| AccountScope::of(&self.principal));
        CallHandle(Arc::new(CallState {
            principal: self.principal,
            request_id: self.request_id,
            mode,
            scope,
            db,
            extra: self.extra,
            services: self.services,
            invoker: self.invoker,
            client: self.client,
            tz: self.tz,
            task: self.task,
            events: Mutex::new(Vec::new()),
            after_commit: Mutex::new(Vec::new()),
            session: Mutex::new(None),
            resolved: Mutex::new(None),
            download: Mutex::new(None),
            lines: Mutex::new(None),
            invoked: Mutex::new(None),
        }))
    }
}

#[derive(Clone)]
pub struct CallHandle(Arc<CallState>);

#[derive(Debug, thiserror::Error)]
#[error("the call is still referenced by {holders} handle(s); the transaction cannot be taken")]
pub struct StillShared {
    pub holders: usize,
}

impl CallHandle {
    pub fn call<P>(&self, params: P) -> Call<P> {
        Call::from_parts(self.0.clone(), params)
    }

    pub fn principal(&self) -> &Principal {
        &self.0.principal
    }

    pub fn request_id(&self) -> &str {
        &self.0.request_id
    }

    pub fn mode(&self) -> Mode {
        self.0.mode
    }

    pub fn account_scope(&self) -> &AccountScope {
        &self.0.scope
    }

    pub fn db(&self) -> &Db {
        &self.0.db
    }

    pub fn extra(&self) -> &Map<String, Value> {
        &self.0.extra
    }

    pub fn client(&self) -> Option<&Client> {
        self.0.client.as_ref()
    }

    pub fn tz(&self) -> Tz {
        self.0.tz
    }

    pub fn task(&self) -> Option<&TaskHandle> {
        self.0.task.as_ref()
    }

    pub fn take_session_change(&self) -> Option<SessionChange> {
        self.0
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    pub fn after_context(&self) -> AfterContext {
        AfterContext {
            principal: self.0.principal.clone(),
            request_id: self.0.request_id.clone(),
            db: None,
        }
    }

    pub fn take_events(&self) -> Vec<Emitted> {
        std::mem::take(&mut *self.0.events.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn resolved_account(&self) -> Option<Id> {
        *self
            .0
            .resolved
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn take_download(&self) -> Option<Download> {
        self.0
            .download
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    pub fn take_lines(&self) -> Option<Lines> {
        self.0
            .lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    pub fn set_invoked(&self, invoked: Invoked) {
        *self
            .0
            .invoked
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(invoked);
    }

    pub fn take_after_commit(&self) -> Vec<AfterCommitHook> {
        std::mem::take(
            &mut *self
                .0
                .after_commit
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    pub fn finish(self) -> Result<Option<DatabaseTransaction>, StillShared> {
        let holders = Arc::strong_count(&self.0);
        let state = Arc::try_unwrap(self.0).map_err(|_| StillShared { holders })?;
        state
            .db
            .into_transaction()
            .map_err(|holders| StillShared { holders })
    }
}

impl fmt::Debug for CallHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallHandle")
            .field("principal", &self.0.principal.login)
            .field("request_id", &self.0.request_id)
            .field("mode", &self.0.mode)
            .finish()
    }
}
