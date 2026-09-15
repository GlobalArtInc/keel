use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::entity::{delivery, subscription};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    pub subscriptions: u64,
    pub deliveries: u64,
}

pub async fn drop_account(db: &Db, account: Id) -> Result<Dropped> {
    let account = account.to_string();
    let deliveries = delivery::Entity::delete_many()
        .filter(delivery::Column::AccountId.eq(account.clone()))
        .exec(db)
        .await?;
    let subscriptions = subscription::Entity::delete_many()
        .filter(subscription::Column::AccountId.eq(account))
        .exec(db)
        .await?;
    Ok(Dropped {
        subscriptions: subscriptions.rows_affected,
        deliveries: deliveries.rows_affected,
    })
}
