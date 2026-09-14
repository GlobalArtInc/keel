pub mod functions;

use std::collections::BTreeMap;

use keel_sdk::async_trait::async_trait;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::PermissionSource;
use keel_sdk::{Db, Id, Result};

use super::entity::grant;

pub const PERMISSION_READ: &str = "auth.grant.read";
pub const PERMISSION_MANAGE: &str = "auth.grant.manage";

#[derive(Clone, Copy, Debug, Default)]
pub struct Grants;

#[async_trait]
impl PermissionSource for Grants {
    async fn grants(&self, principal: Id, db: &Db) -> Result<BTreeMap<String, bool>> {
        let rows = grant::Entity::find()
            .filter(grant::Column::PrincipalId.eq(principal.to_string()))
            .all(db)
            .await?;
        Ok(rows.into_iter().map(|row| (row.key, row.granted)).collect())
    }
}
