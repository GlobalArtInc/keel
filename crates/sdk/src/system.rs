use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use keel_types::{CallError, Mode, Principal};
use serde::Serialize;
use serde_json::Value;

use crate::db::Db;
use crate::error::Result;
use crate::secret::Secret;

#[derive(Clone, Debug)]
pub enum Credential {
    Bearer(Secret),
    Session(Secret),
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(&self, credential: &Credential, db: &Db) -> Result<Option<Principal>>;
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub request_id: String,
    pub function: String,
    pub module: String,
    pub mode: Mode,
    pub principal: Principal,
    pub params: Value,
    pub outcome: AuditOutcome,
    pub at: DateTime<Utc>,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum AuditOutcome {
    Committed,
    Failed { error: CallError },
}

impl AuditOutcome {
    pub fn is_committed(&self) -> bool {
        matches!(self, AuditOutcome::Committed)
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}
