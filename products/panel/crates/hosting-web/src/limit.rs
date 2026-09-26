use keel::prelude::*;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement,
};
use keel::sdk::serde_json;
use keel_hosting_users::api::HostingUsers;

use crate::{LIMIT, site};

const SERIALISE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('hosting_site:' || $1, 0))";
const BUSY: &str = "hosting.site.error.user_busy";

pub(crate) async fn enforce(call: Call<Value>) -> Result<()> {
    let Ok(user) = serde_json::from_value::<Id>(call.params()["user"].clone()) else {
        return Ok(());
    };
    let tx = call.tx()?;
    let Some(owner) = call.require::<dyn HostingUsers>()?.get(tx, &user).await? else {
        return Ok(());
    };
    if !call.account_scope().includes(tx, owner.account).await? {
        return Ok(());
    }
    let account = call.resolve_account(Some(owner.account)).await?.to_string();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SERIALISE,
        [account.as_str().into()],
    ))
    .await?;
    let kept = site::Entity::find()
        .filter(site::Column::AccountId.eq(account))
        .count(tx)
        .await?;
    call.check_limit(LIMIT, kept).await
}

pub(crate) async fn refuse_busy_users(call: Call<Value>) -> Result<()> {
    let ids: Vec<Id> = serde_json::from_value(call.params()["ids"].clone())?;
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let busy = site::Entity::find()
        .filter(site::Column::UserId.is_in(ids))
        .count(call.tx()?)
        .await?;
    if busy == 0 {
        Ok(())
    } else {
        Err(KeelError::conflict(BUSY).field("ids").param("sites", busy))
    }
}
