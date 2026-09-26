use keel::prelude::*;
use keel::sdk::futures::future::BoxFuture;

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
