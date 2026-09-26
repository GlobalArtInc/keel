use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::api::Databases;
use crate::database::{self, STATE_ACTIVE};

pub(crate) struct Lookup;

#[async_trait]
impl Databases for Lookup {
    async fn of_user(&self, db: &Db, account: &Id, user: &Id) -> Result<Vec<String>> {
        Ok(database::Entity::find()
            .filter(database::Column::AccountId.eq(account.to_string()))
            .filter(database::Column::UserId.eq(user.to_string()))
            .filter(database::Column::State.eq(STATE_ACTIVE))
            .order_by_asc(database::Column::Name)
            .all(db)
            .await?
            .into_iter()
            .map(|row| row.name)
            .collect())
    }
}
