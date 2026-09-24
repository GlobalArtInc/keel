use keel::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneRef {
    pub id: Id,
    pub account: Id,
    pub serial: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel::sdk::serde")]
pub struct RecordLine {
    pub name: String,
    pub ttl: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: String,
}

#[async_trait]
pub trait Zones: Send + Sync {
    async fn change(
        &self,
        db: &Db,
        scope: &AccountScope,
        zone: &Id,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<ZoneRef>;

    async fn publish(&self, ctx: &AfterContext, zone: &Id) -> Result<()>;
}

#[async_trait]
pub trait ZoneRecords: Send + Sync {
    async fn lines(&self, db: &Db, zone: &Id) -> Result<Vec<RecordLine>>;
}
