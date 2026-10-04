use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::functions::remove_after_commit;
use crate::{account, service, switch, zone};

pub(crate) fn contribution() -> AccountLifecycle {
    AccountLifecycle::new()
        .on_suspend(switch_zones)
        .on_resume(switch_zones)
        .on_delete(drop_zones)
}

async fn switch_zones(call: Call<Id>) -> Result<()> {
    switch::switch(&call, *call.params()).await
}

async fn drop_zones(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let rows = zone::Entity::find()
        .filter(zone::Column::AccountId.eq(account.as_str()))
        .all(tx)
        .await?;
    zone::Entity::delete_many()
        .filter(zone::Column::AccountId.eq(account.as_str()))
        .exec(tx)
        .await?;
    service::Entity::delete_many()
        .filter(service::Column::AccountId.eq(account.as_str()))
        .exec(tx)
        .await?;
    account::Entity::delete_by_id(account.as_str())
        .exec(tx)
        .await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "dropped {} zone(s) of account {account}",
            rows.len()
        ))
        .await?;
    }
    remove_after_commit(&call, rows)
}
