use keel::prelude::*;

use crate::zone;

#[async_trait]
pub(crate) trait DnsProvider: Send + Sync {
    async fn apply(&self, ctx: &AfterContext, zone: &str) -> Result<()>;
    async fn remove(&self, ctx: &AfterContext, zones: &[zone::Model]) -> Result<()>;
    async fn reindex(&self, ctx: &AfterContext) -> Result<()>;
}
