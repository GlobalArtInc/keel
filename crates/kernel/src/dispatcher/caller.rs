use std::fmt;
use std::sync::{Mutex, PoisonError};

use keel_sdk::KeelError;
use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Caller, CallerOptions};
use keel_types::{Envelope, Principal, Via};
use serde_json::Value;

use super::{CallOptions, Dispatcher};
use crate::grants;

#[derive(Default)]
pub struct CallerHandle {
    dispatcher: Mutex<Option<Dispatcher>>,
}

impl CallerHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fill(&self, dispatcher: Dispatcher) {
        *self
            .dispatcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(dispatcher);
    }

    pub fn is_filled(&self) -> bool {
        self.dispatcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn dispatcher(&self) -> Option<Dispatcher> {
        self.dispatcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Caller for CallerHandle {
    async fn call(
        &self,
        mut principal: Principal,
        func: &str,
        params: Value,
        options: CallerOptions,
    ) -> Result<Value, KeelError> {
        let Some(dispatcher) = self.dispatcher() else {
            return Err(
                KeelError::unavailable("keel.error.caller_unavailable").param("function", func)
            );
        };
        principal.via = Via::Internal;
        let db = Db::from_connection(dispatcher.db().clone());
        grants::overlay(dispatcher.registry(), &mut principal, &db).await?;
        let reply = dispatcher
            .call(
                principal,
                func,
                params,
                CallOptions {
                    request_id: options.request_id,
                    task: options.task,
                    ..CallOptions::default()
                },
            )
            .await;
        match reply.envelope {
            Envelope::Ok { data, .. } => Ok(data),
            Envelope::Err { error } => Err(error.into()),
        }
    }
}

impl fmt::Debug for CallerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallerHandle")
            .field("filled", &self.is_filled())
            .finish()
    }
}
