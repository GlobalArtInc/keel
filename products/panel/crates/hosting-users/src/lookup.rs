use keel::prelude::*;
use keel::sdk::sea_orm::EntityTrait;

use crate::api::{HostingUser, HostingUsers};
use crate::user;

pub(crate) struct Lookup;

#[async_trait]
impl HostingUsers for Lookup {
    async fn get(&self, db: &Db, id: &Id) -> Result<Option<HostingUser>> {
        let Some(row) = user::Entity::find_by_id(id.to_string()).one(db).await? else {
            return Ok(None);
        };
        let account = row.account_id.parse().map_err(|_| {
            KeelError::internal("hosting.user.error.account")
                .param("account", row.account_id.as_str())
        })?;
        Ok(Some(HostingUser {
            id: *id,
            account,
            name: row.name,
            home: row.home.filter(|_| row.uid.is_some()),
        }))
    }
}
