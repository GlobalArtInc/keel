use keel::prelude::*;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement,
};
use keel::sdk::serde_json;

use crate::{LIMIT, user};

const SERIALISE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('hosting_user:' || $1, 0))";

pub(crate) async fn enforce(call: Call<Value>) -> Result<()> {
    let requested: Option<Id> = serde_json::from_value(call.params()["account"].clone())?;
    let account = call.resolve_account(requested).await?.to_string();
    let tx = call.tx()?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SERIALISE,
        [account.as_str().into()],
    ))
    .await?;
    let kept = user::Entity::find()
        .filter(user::Column::AccountId.eq(account))
        .count(tx)
        .await?;
    call.check_limit(LIMIT, kept).await
}
