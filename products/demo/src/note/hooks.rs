use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use keel::sdk::serde_json;

use super::{Batch, LOCKED_PREFIX, MAX_PER_ACCOUNT, item};

pub async fn refuse_locked(call: Call<Value>) -> Result<Before> {
    let batch: Batch = serde_json::from_value(call.params().clone())?;
    let ids: Vec<String> = batch.ids.iter().map(ToString::to_string).collect();
    let locked = item::Entity::find()
        .filter(item::Column::Id.is_in(ids))
        .filter(item::Column::Title.starts_with(LOCKED_PREFIX))
        .one(call.db())
        .await?;
    match locked {
        Some(row) => Err(KeelError::conflict("note.error.locked")
            .field("ids")
            .param("title", row.title)),
        None => Ok(Before::Proceed),
    }
}

pub async fn enforce_limit(call: Call<Value>) -> Result<()> {
    if !call.params()["id"].is_null() {
        return Ok(());
    }
    let requested: Option<Id> = serde_json::from_value(call.params()["account"].clone())?;
    let account = call.resolve_account(requested).await?;
    let kept = item::Entity::find()
        .filter(item::Column::AccountId.eq(account.to_string()))
        .count(call.tx()?)
        .await?;
    if kept >= MAX_PER_ACCOUNT {
        return Err(KeelError::limit_exceeded("note.error.limit").param("max", MAX_PER_ACCOUNT));
    }
    Ok(())
}
