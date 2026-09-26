use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::api::{HostingUser, HostingUsers};
use crate::user;

pub(crate) struct Lookup;

#[async_trait]
impl HostingUsers for Lookup {
    async fn get(&self, db: &Db, id: &Id) -> Result<Option<HostingUser>> {
        user::Entity::find_by_id(id.to_string())
            .one(db)
            .await?
            .map(hosting_user)
            .transpose()
    }

    async fn named(&self, db: &Db, name: &str) -> Result<Option<HostingUser>> {
        user::Entity::find()
            .filter(user::Column::Name.eq(name))
            .one(db)
            .await?
            .map(hosting_user)
            .transpose()
    }
}

fn hosting_user(row: user::Model) -> Result<HostingUser> {
    let id = row
        .id
        .parse()
        .map_err(|_| KeelError::internal("hosting.user.error.id").param("id", row.id.as_str()))?;
    let account = row.account_id.parse().map_err(|_| {
        KeelError::internal("hosting.user.error.account").param("account", row.account_id.as_str())
    })?;
    Ok(HostingUser {
        id,
        account,
        name: row.name,
        home: row.home.filter(|_| row.uid.is_some()),
    })
}
