use keel::prelude::*;
use keel::sdk::futures::future::BoxFuture;
use keel_host::api::Origin;

pub type Invoke<'a> = dyn Fn(&str, Value) -> BoxFuture<'static, Result<Value>> + Send + Sync + 'a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiteZone {
    pub name: String,
    pub account: Id,
    pub own_account: bool,
}

#[async_trait]
pub trait DnsProvider: Send + Sync {
    async fn add_zone(&self, invoke: &Invoke<'_>, zone: &SiteZone) -> Result<()>;
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

#[async_trait]
pub trait Sites: Send + Sync {
    async fn render(&self, db: &Db, origin: Origin, site: &Id) -> Result<()>;
}
