use keel::prelude::*;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement,
};
use keel::sdk::serde_json;

use crate::names::owner;
use crate::{DATABASE_LIMIT, LOGIN_LIMIT, database, login};

const SERIALISE: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('hosting_database:' || $1, 0))";
const BUSY: &str = "hosting.database.error.user_busy";

pub(crate) async fn databases(call: Call<Value>) -> Result<()> {
    let Some(account) = serialised(&call).await? else {
        return Ok(());
    };
    let kept = database::Entity::find()
        .filter(database::Column::AccountId.eq(account))
        .count(call.tx()?)
        .await?;
    call.check_limit(DATABASE_LIMIT, kept).await
}

pub(crate) async fn logins(call: Call<Value>) -> Result<()> {
    let Some(account) = serialised(&call).await? else {
        return Ok(());
    };
    let kept = login::Entity::find()
        .filter(login::Column::AccountId.eq(account))
        .count(call.tx()?)
        .await?;
    call.check_limit(LOGIN_LIMIT, kept).await
}

async fn serialised(call: &Call<Value>) -> Result<Option<String>> {
    let Some(name) = call.params()["name"].as_str() else {
        return Ok(None);
    };
    let Some(owner) = owner(call, name).await? else {
        return Ok(None);
    };
    let account = call.resolve_account(Some(owner.account)).await?.to_string();
    call.tx()?
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            SERIALISE,
            [account.as_str().into()],
        ))
        .await?;
    Ok(Some(account))
}

pub(crate) async fn refuse_busy_users(call: Call<Value>) -> Result<()> {
    let ids: Vec<Id> = serde_json::from_value(call.params()["ids"].clone())?;
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let tx = call.tx()?;
    let databases = database::Entity::find()
        .filter(database::Column::UserId.is_in(ids.clone()))
        .count(tx)
        .await?;
    let logins = login::Entity::find()
        .filter(login::Column::UserId.is_in(ids))
        .count(tx)
        .await?;
    if databases == 0 && logins == 0 {
        Ok(())
    } else {
        Err(KeelError::conflict(BUSY)
            .field("ids")
            .param("databases", databases)
            .param("users", logins))
    }
}
