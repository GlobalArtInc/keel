use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use keel_types::{
    Account, BootLicense, BootProblems, CallError, DashboardLayout, DownloadFile, Event, Id, Level,
    Mode, Principal, TaskState,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::watch;

use crate::call::Call;
use crate::context::Context;
use crate::db::Db;
use crate::error::Result;
use crate::product::ProductManifest;
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
pub trait OnBehalf: Send + Sync {
    async fn act_for(&self, caller: &Principal, header: &str, db: &Db)
    -> Result<Option<Principal>>;
}

#[async_trait]
pub trait PeerReach: Send + Sync {
    async fn reaches(&self, link: Id, principal: &Principal, db: &Db) -> Result<bool>;
    async fn customer(&self, link: Id, external_id: &str, db: &Db) -> Result<Option<Principal>>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPrincipal {
    pub account: Id,
    pub login: String,
    pub level: Level,
    pub role: String,
    pub lang: String,
}

#[async_trait]
pub trait PrincipalSource: Send + Sync {
    async fn by_id(&self, id: Id, db: &Db) -> Result<Option<Principal>>;
    async fn by_login(&self, login: &str, db: &Db) -> Result<Option<Principal>>;
    async fn by_external(
        &self,
        origin: &str,
        external_id: &str,
        db: &Db,
    ) -> Result<Option<Principal>>;
    async fn of_external_id(&self, external_id: &str, db: &Db) -> Result<Vec<Principal>>;
    async fn at_or_above(&self, level: Level, db: &Db) -> Result<Vec<Principal>>;
    async fn of_account(&self, account: Id, db: &Db) -> Result<Vec<Principal>>;
    async fn touch_acl(&self, id: Id, db: &Db) -> Result<()>;
    async fn create(
        &self,
        principal: NewPrincipal,
        now: DateTime<Utc>,
        db: &Db,
    ) -> Result<Principal>;
}

#[async_trait]
pub trait ProblemCounter: Send + Sync {
    async fn counts(&self, principal: &Principal, db: &Db) -> Result<BootProblems>;
}

#[async_trait]
pub trait PermissionSource: Send + Sync {
    async fn grants(&self, principal: Id, db: &Db) -> Result<BTreeMap<String, bool>>;
}

pub trait PermissionCatalogue: Send + Sync {
    fn registered(&self, key: &str) -> bool;
    fn holds(&self, principal: &Principal, key: &str) -> bool;
}

pub trait Features: Send + Sync {
    fn enabled(&self) -> Vec<String>;
    fn allows(&self, feature: &str) -> bool;

    fn edition(&self, product: &ProductManifest) -> Option<String> {
        product
            .default_edition()
            .map(|edition| edition.id.to_owned())
    }

    fn license(&self) -> Option<BootLicense> {
        None
    }

    fn degradation(&self, _module: &str) -> Option<Degradation> {
        None
    }

    fn may_enable(&self, _product: &str) -> bool {
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Degradation {
    ReadOnly,
    Unlisted { product: String },
}

pub trait LicenseKey: Send + Sync {
    fn open(&self, signed: &[u8]) -> std::result::Result<Vec<u8>, String>;
}

#[derive(Clone, Debug)]
pub struct Installation {
    pub instance: Option<Id>,
    pub products: Vec<&'static ProductManifest>,
}

#[async_trait]
pub trait AccountSource: Send + Sync {
    async fn ancestry(&self, account: Id, db: &Db) -> Result<Vec<Id>>;
    async fn by_id(&self, account: Id, db: &Db) -> Result<Option<Account>>;
    async fn of_external_id(&self, external_id: &str, db: &Db) -> Result<Vec<Account>>;
}

#[async_trait]
pub trait LimitSource: Send + Sync {
    async fn effective(&self, key: &str, account: Id, db: &Db) -> Result<Option<u64>>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub lang: Option<String>,
    pub tz: Option<String>,
}

#[async_trait]
pub trait PreferenceSource: Send + Sync {
    async fn preferences(&self, principal: Id, db: &Db) -> Result<Preferences>;
    async fn dashboard(&self, principal: Id, db: &Db) -> Result<Option<DashboardLayout>>;
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

#[async_trait]
pub trait PasswordStore: Send + Sync {
    async fn set(&self, principal: Id, login: &str, password: &Secret, db: &Db) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub value: String,
    pub verified: bool,
}

#[async_trait]
pub trait AddressSource: Send + Sync {
    async fn email(&self, principal: Id, db: &Db) -> Result<Option<Address>>;
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
    async fn enqueue(&self, db: &Db, spec: TaskSpec) -> Result<Id>;
    async fn wake(&self, id: Id) -> Result<()>;
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
    async fn quiesce(&self, except: Id) -> Result<Vec<Id>>;
    fn resume(&self) -> Result<()>;
}

pub trait ParamMasker: Send + Sync {
    fn mask(&self, func: &str, params: &mut Value) -> Result<()>;
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
    fn stored(&self, file: DownloadFile);
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub keel: String,
    pub at: DateTime<Utc>,
    pub instance: Option<String>,
    pub schema: String,
    pub products: Vec<BackupProduct>,
    pub modules: Vec<BackupModule>,
    pub tables: Vec<BackupTable>,
    pub sources: Vec<String>,
    pub config: Vec<String>,
    pub data: Vec<String>,
    pub secret_key: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupProduct {
    pub id: String,
    pub version: String,
    pub edition: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupModule {
    pub id: String,
    pub version: String,
    pub ledger: Option<String>,
    pub applied: u32,
    pub head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupTable {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupCreated {
    pub archive: PathBuf,
    pub bytes: u64,
    pub manifest: BackupManifest,
}

#[async_trait]
pub trait Backups: Send + Sync {
    fn directory(&self) -> PathBuf;
    async fn create(&self, archive: &Path) -> Result<BackupCreated>;
    async fn inspect(&self, archive: &Path) -> Result<BackupManifest>;
    fn admits(&self, manifest: &BackupManifest) -> Result<()>;
    async fn request_restore(&self, archive: &Path, requested_by: &str) -> Result<PathBuf>;
}

#[async_trait]
pub trait OutboxSubscriber: Send + Sync {
    async fn after(&self, seq: u64, limit: u32) -> Result<Vec<Event>>;
    async fn newest(&self) -> Result<u64>;
    fn latest(&self) -> watch::Receiver<u64>;
}

#[async_trait]
pub trait EventRelay: Send + Sync {
    async fn relay(&self, event: &Event) -> Result<Option<u64>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndpointMethod {
    Get,
    Post,
}

impl EndpointMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            EndpointMethod::Get => "GET",
            EndpointMethod::Post => "POST",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EndpointRequest {
    pub principal: Principal,
    pub client: Client,
    pub request_id: String,
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl EndpointRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn param(&self, name: &str) -> impl Iterator<Item = &str> {
        self.query
            .iter()
            .filter(move |(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

pub type EndpointHandler =
    Arc<dyn Fn(EndpointRequest) -> BoxFuture<'static, Result<Value>> + Send + Sync>;

#[derive(Clone)]
pub struct Endpoint {
    module: String,
    name: String,
    method: EndpointMethod,
    handler: EndpointHandler,
}

impl Endpoint {
    pub fn new<F, Fut>(method: EndpointMethod, name: &str, handler: F) -> Self
    where
        F: Fn(EndpointRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self {
            module: String::new(),
            name: name.to_owned(),
            method,
            handler: Arc::new(move |request| Box::pin(handler(request))),
        }
    }

    pub fn get<F, Fut>(name: &str, handler: F) -> Self
    where
        F: Fn(EndpointRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self::new(EndpointMethod::Get, name, handler)
    }

    pub fn post<F, Fut>(name: &str, handler: F) -> Self
    where
        F: Fn(EndpointRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        Self::new(EndpointMethod::Post, name, handler)
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn method(&self) -> EndpointMethod {
        self.method
    }

    pub fn serve(&self, request: EndpointRequest) -> BoxFuture<'static, Result<Value>> {
        (self.handler)(request)
    }

    pub(crate) fn stamp(&mut self, module: &str) {
        module.clone_into(&mut self.module);
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("module", &self.module)
            .field("name", &self.name)
            .field("method", &self.method)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::CallBuilder;

    #[tokio::test]
    async fn an_endpoint_request_reads_headers_regardless_of_case_and_repeated_query_values() {
        let request = EndpointRequest {
            principal: crate::tests::principal(Level::Guest),
            client: Client::default(),
            request_id: "req-1".into(),
            headers: vec![
                ("x-keel-peer".into(), "01J9B".into()),
                ("x-keel-peer".into(), "second".into()),
            ],
            query: vec![
                ("types".into(), "dns.zone.*".into()),
                ("after".into(), "7".into()),
                ("types".into(), "svc.*".into()),
            ],
            body: b"{}".to_vec(),
        };
        assert_eq!(request.header("X-Keel-Peer"), Some("01J9B"));
        assert_eq!(request.header("x-keel-signature"), None);
        assert_eq!(
            request.param("types").collect::<Vec<_>>(),
            ["dns.zone.*", "svc.*"]
        );
        assert_eq!(request.param("limit").count(), 0);
        let endpoint = Endpoint::post("hook", |request: EndpointRequest| async move {
            Ok(serde_json::json!({ "bytes": request.body.len() }))
        });
        assert_eq!(endpoint.method(), EndpointMethod::Post);
        assert_eq!(endpoint.name(), "hook");
        assert_eq!(endpoint.module(), "");
        assert_eq!(
            endpoint.serve(request).await.unwrap(),
            serde_json::json!({ "bytes": 2 })
        );
        assert_eq!(EndpointMethod::Get.as_str(), "GET");
        assert_eq!(
            format!("{endpoint:?}"),
            "Endpoint { module: \"\", name: \"hook\", method: Post }"
        );
    }

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
