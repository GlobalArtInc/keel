use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use keel::sdk::serde_json;

use crate::switch::serialise;
use crate::{LIMIT, zone};

pub(crate) async fn enforce_limit(call: Call<Value>) -> Result<()> {
    let requested: Option<Id> = serde_json::from_value(call.params()["account"].clone())?;
    let account = call.resolve_account(requested).await?.to_string();
    let tx = call.tx()?;
    serialise(tx, &account).await?;
    let kept = zone::Entity::find()
        .filter(zone::Column::AccountId.eq(account))
        .count(tx)
        .await?;
    call.check_limit(LIMIT, kept).await
}
