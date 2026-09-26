use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel::sdk::serde_json;

use crate::{backup, run, schedule};

pub(crate) fn contribution() -> AccountLifecycle {
    AccountLifecycle::new().on_delete(|call: Call<Id>| async move {
        let account = call.params().to_string();
        let tx = call.tx()?;
        schedule::Entity::delete_many()
            .filter(schedule::Column::AccountId.eq(account.as_str()))
            .exec(tx)
            .await?;
        let files = run::drop_backups(tx, backup::Column::AccountId.eq(account.as_str())).await?;
        if let Some(task) = call.task() {
            task.log(&format!(
                "removed {} site backup(s) of account {account}",
                files.len()
            ))
            .await?;
        }
        run::forget_after_commit(&call, files)
    })
}

pub(crate) async fn forget_sites(call: Call<Value>) -> Result<()> {
    let ids: Vec<Id> = serde_json::from_value(call.params()["ids"].clone())?;
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let tx = call.tx()?;
    schedule::Entity::delete_many()
        .filter(schedule::Column::SiteId.is_in(ids.clone()))
        .exec(tx)
        .await?;
    let files = run::drop_backups(tx, backup::Column::SiteId.is_in(ids)).await?;
    run::forget_after_commit(&call, files)
}
