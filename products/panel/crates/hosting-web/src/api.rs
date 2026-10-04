use keel::prelude::*;
use keel::sdk::futures::future::BoxFuture;
use keel_host::api::Origin;

pub type Invoke<'a> = dyn Fn(&str, Value) -> BoxFuture<'static, Result<Value>> + Send + Sync + 'a;

pub type Deferred = Box<dyn FnOnce(AfterContext) -> BoxFuture<'static, Result<()>> + Send>;

pub type AfterCommit<'a> = dyn Fn(Deferred) + Send + Sync + 'a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteZone {
    pub name: String,
    pub account: Id,
    pub own_account: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneRecord {
    pub kind: &'static str,
    pub name: String,
    pub value: String,
    pub priority: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneFailure {
    pub site: Id,
    pub account: Id,
    pub domain: String,
    pub error: String,
}

#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn add_zone(
        &self,
        invoke: &Invoke<'_>,
        after: &AfterCommit<'_>,
        site: &Id,
        zone: &SiteZone,
    ) -> Result<()>;
    async fn foreign(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<bool>;
    async fn add_records(
        &self,
        invoke: &Invoke<'_>,
        zone: &SiteZone,
        records: &[ZoneRecord],
    ) -> Result<bool>;
    async fn open(&self, db: &Db, zone: &SiteZone) -> Result<String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TlsPair {
    pub certificate: String,
    pub key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteTls {
    pub challenges: String,
    pub pair: Option<TlsPair>,
}

#[async_trait]
pub trait SiteCertificates: Send + Sync {
    async fn tls(&self, db: &Db, site: &Id) -> Result<SiteTls>;
    async fn request(&self, invoke: &Invoke<'_>, site: &Id) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentRoot {
    pub user: String,
    pub path: String,
    pub home: String,
    pub root: String,
}

#[async_trait]
pub trait Sites: Send + Sync {
    async fn render(&self, db: &Db, origin: Origin, site: &Id) -> Result<()>;
    async fn document_root(&self, db: &Db, site: &Id) -> Result<Option<DocumentRoot>>;
    async fn taken(&self, db: &Db, names: &[String]) -> Result<Option<String>>;
    async fn zone_failed(&self, db: &Db, site: &Id, error: Option<&str>) -> Result<()>;
    async fn zone_failures(&self, db: &Db) -> Result<Vec<ZoneFailure>>;
}
