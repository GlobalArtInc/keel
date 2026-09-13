use std::any::type_name;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};

use futures::future::BoxFuture;
use keel_types::{Id, Mode, Principal};
use sea_orm::{DatabaseConnection, DatabaseTransaction};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::access::AccountScope;
use crate::db::Db;
use crate::error::{KeelError, Result};
use crate::services::Services;
use crate::system::{Client, SessionChange};

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
    events: Mutex<Vec<Emitted>>,
    after_commit: Mutex<Vec<AfterCommitHook>>,
    session: Mutex<Option<SessionChange>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Emitted {
    pub kind: String,
    pub data: Value,
}

pub type AfterCommitHook = Box<dyn FnOnce(AfterContext) -> BoxFuture<'static, Result<()>> + Send>;

#[derive(Clone, Debug)]
pub struct AfterContext {
    principal: Principal,
    request_id: String,
}

impl AfterContext {
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
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

    pub async fn resolve_account(&self, requested: Option<Id>) -> Result<Id> {
        let own = self.state.principal.account_id;
        let Some(requested) = requested else {
            return Ok(own);
        };
        if requested == own || self.state.scope.includes(&self.state.db, requested).await? {
            Ok(requested)
        } else {
            Err(KeelError::not_found("keel.error.account_not_found").field("account"))
        }
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
        }
    }

    pub fn client(mut self, client: Option<Client>) -> Self {
        self.client = client;
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
            events: Mutex::new(Vec::new()),
            after_commit: Mutex::new(Vec::new()),
            session: Mutex::new(None),
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
        }
    }

    pub fn take_events(&self) -> Vec<Emitted> {
        std::mem::take(&mut *self.0.events.lock().unwrap_or_else(PoisonError::into_inner))
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
