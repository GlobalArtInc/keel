use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use keel_types::{CallError, Id, Mode, Principal, TaskState};
use serde::Serialize;
use serde_json::Value;

use crate::call::Call;
use crate::context::Context;
use crate::db::Db;
use crate::error::Result;
use crate::secret::Secret;
use crate::task::TaskHandle;

#[derive(Clone, Debug)]
pub enum Credential {
    Bearer(Secret),
    Session(Secret),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Client {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub session: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Authenticated {
    pub principal: Principal,
    pub session: Option<String>,
}

#[derive(Clone, Debug)]
pub enum SessionChange {
    Set(Secret),
    Clear,
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(
        &self,
        credential: &Credential,
        client: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>>;
}

#[async_trait]
pub trait PrincipalSource: Send + Sync {
    async fn by_id(&self, id: Id, db: &Db) -> Result<Option<Principal>>;
    async fn by_login(&self, login: &str, db: &Db) -> Result<Option<Principal>>;
    async fn touch_acl(&self, id: Id, db: &Db) -> Result<()>;
}

#[async_trait]
pub trait PermissionSource: Send + Sync {
    async fn grants(&self, principal: Id, db: &Db) -> Result<BTreeMap<String, bool>>;
}

pub trait PermissionCatalogue: Send + Sync {
    fn registered(&self, key: &str) -> bool;
    fn holds(&self, principal: &Principal, key: &str) -> bool;
}

#[async_trait]
pub trait AccountSource: Send + Sync {
    async fn ancestry(&self, account: Id, db: &Db) -> Result<Vec<Id>>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub lang: Option<String>,
    pub tz: Option<String>,
}

#[async_trait]
pub trait PreferenceSource: Send + Sync {
    async fn preferences(&self, principal: Id, db: &Db) -> Result<Preferences>;
}

pub trait LanguageSource: Send + Sync {
    fn languages(&self) -> Vec<String>;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ManifestPatch {
    pub name: Option<String>,
    pub logo: Option<String>,
    pub hue: Option<u16>,
    pub chroma: Option<f64>,
}

impl ManifestPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.logo.is_none() && self.hue.is_none() && self.chroma.is_none()
    }
}

#[async_trait]
pub trait ManifestSource: Send + Sync {
    async fn patch(&self, host: &str, db: &Db) -> Result<Option<ManifestPatch>>;
}

pub trait SecretCipher: Send + Sync {
    fn seal(&self, plaintext: &[u8]) -> Result<String>;
    fn open(&self, sealed: &str) -> Result<Vec<u8>>;
}

#[async_trait]
pub trait SecondFactorSource: Send + Sync {
    async fn enrolled(&self, principal: Id, db: &Db) -> Result<bool>;
}

pub trait CallExt {
    fn client(&self) -> Option<&Client>;
    fn session(&self, change: SessionChange);
}

pub trait ContextExt {
    fn db(&self) -> Db;
    fn data_dir(&self) -> &Path;
}

impl ContextExt for Context {
    fn db(&self) -> Db {
        self.db_slot().clone()
    }

    fn data_dir(&self) -> &Path {
        self.data_dir_slot()
    }
}

impl<P> CallExt for Call<P> {
    fn client(&self) -> Option<&Client> {
        self.client_slot()
    }

    fn session(&self, change: SessionChange) {
        self.record_session(change);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub request_id: String,
    pub function: String,
    pub module: String,
    pub mode: Mode,
    pub principal: Principal,
    pub client: Option<Client>,
    pub params: Value,
    pub subject: Option<String>,
    pub result: Option<Value>,
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

#[derive(Clone, Debug, PartialEq)]
pub struct TaskSpec {
    pub function: String,
    pub params: Value,
    pub principal: Principal,
    pub class: String,
    pub resumable: bool,
    pub request_id: Option<String>,
}

#[async_trait]
pub trait TaskRunner: Send + Sync {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id>;
    async fn progress(
        &self,
        id: Id,
        done: u64,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<()>;
    async fn log(&self, id: Id, line: &str) -> Result<()>;
    async fn checkpoint(&self, id: Id, state: Value) -> Result<()>;
    async fn cancel(&self, id: Id) -> Result<()>;
    async fn state(&self, id: Id) -> Result<TaskState>;
}

#[async_trait]
pub trait TaskSink: Send + Sync {
    fn id(&self) -> Id;
    async fn progress(&self, done: u64, total: Option<u64>, message: Option<&str>) -> Result<()>;
    async fn log(&self, line: &str) -> Result<()>;
    async fn checkpoint(&self, state: Value) -> Result<()>;
    fn resume_from(&self) -> Option<Value>;
    fn is_cancelled(&self) -> bool;
    fn output_path(&self) -> PathBuf;
}

#[derive(Clone, Debug, Default)]
pub struct CallerOptions {
    pub request_id: Option<String>,
    pub task: Option<TaskHandle>,
}

#[async_trait]
pub trait Caller: Send + Sync {
    async fn call(
        &self,
        principal: Principal,
        func: &str,
        params: Value,
        options: CallerOptions,
    ) -> Result<Value>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub principal: Id,
    pub account: Id,
    pub kind: String,
    pub data: Value,
}

pub trait LiveEvents: Send + Sync {
    fn publish(&self, frame: Frame);
}

pub struct LockGuard {
    held: Box<dyn Any + Send>,
}

impl LockGuard {
    pub fn new(held: impl Any + Send) -> Self {
        Self {
            held: Box::new(held),
        }
    }

    pub fn held<T: Any>(&self) -> Option<&T> {
        self.held.downcast_ref::<T>()
    }
}

impl fmt::Debug for LockGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LockGuard")
    }
}

#[async_trait]
pub trait AdvisoryLock: Send + Sync {
    async fn take(&self, key: i64) -> Result<LockGuard>;
}

#[cfg(test)]
mod tests {
    use keel_types::Level;

    use super::*;
    use crate::call::CallBuilder;

    #[test]
    fn a_call_carries_the_client_of_its_request_and_records_one_session_change() {
        let client = Client {
            ip: Some("127.0.0.1".parse().expect("an address")),
            user_agent: Some("probe/1".into()),
            session: Some("abc".into()),
        };
        let handle = CallBuilder::new(crate::tests::principal(Level::User), "req-1")
            .client(Some(client.clone()))
            .tz(chrono_tz::Asia::Tokyo)
            .read(sea_orm::DatabaseConnection::Disconnected);
        let call = handle.call(());
        assert_eq!(call.client(), Some(&client));
        assert_eq!(call.tz(), chrono_tz::Asia::Tokyo);
        assert_eq!(handle.tz(), chrono_tz::Asia::Tokyo);
        assert!(handle.take_session_change().is_none());
        call.session(SessionChange::Set(Secret::new("ks_1")));
        call.session(SessionChange::Clear);
        assert!(matches!(
            handle.take_session_change(),
            Some(SessionChange::Clear)
        ));
        assert!(handle.take_session_change().is_none());
        let bare = crate::tests::read_handle();
        assert!(bare.call(()).client().is_none());
        assert_eq!(bare.call(()).tz(), chrono_tz::UTC);
        assert_eq!(
            format!("{:?}", SessionChange::Set(Secret::new("ks_1"))),
            "Set(Secret(***))"
        );
    }
}
