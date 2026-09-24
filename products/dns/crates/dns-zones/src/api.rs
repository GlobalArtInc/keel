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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expansion {
    pub zone: String,
    pub ip: Option<String>,
    pub ip6: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel::sdk::serde")]
pub struct RecordSeed {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub value: String,
    pub ttl: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[async_trait]
pub trait ZoneTemplates: Send + Sync {
    async fn expand(
        &self,
        db: &Db,
        scope: &AccountScope,
        template: &Id,
        expansion: &Expansion,
    ) -> Result<Vec<RecordSeed>>;
}
