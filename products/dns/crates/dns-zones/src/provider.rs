use keel::prelude::*;

use crate::zone;

#[async_trait]
pub(crate) trait DnsProvider: Send + Sync {
    async fn apply(&self, ctx: &AfterContext, zone: &zone::Model) -> Result<()>;
    async fn remove(&self, ctx: &AfterContext, zones: &[zone::Model]) -> Result<()>;
}
