use keel::sdk::Owned;
use keel::sdk::schemars::JsonSchema;
use keel::sdk::sea_orm;
use keel::sdk::sea_orm::entity::prelude::*;
use keel::sdk::serde::{Deserialize, Serialize};

pub(crate) const KIND_ACME: &str = "acme";
pub(crate) const KIND_UPLOADED: &str = "uploaded";
pub(crate) const STATE_PENDING: &str = "pending";
pub(crate) const STATE_ACTIVE: &str = "active";
pub(crate) const STATE_ERROR: &str = "error";

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
#[schemars(
    crate = "keel::sdk::schemars",
    rename = "HostingCertificate",
    rename_all = "camelCase"
)]
#[sea_orm(table_name = "hosting_certificate")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub account_id: String,
    pub site_id: String,
    pub domain: String,
    pub kind: String,
    pub state: String,
    pub error: Option<String>,
    #[serde(skip)]
    pub installed: Option<String>,
    pub not_after: Option<DateTimeUtc>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

impl Owned for Entity {
    fn account_column() -> Self::Column {
        Column::AccountId
    }
}
