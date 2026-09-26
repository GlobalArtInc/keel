use keel::prelude::*;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, QueryFilter, Statement,
};
use keel::sdk::serde_json;

use crate::{DOMAIN_LIMIT, MAILBOX_LIMIT, domain, mailbox, mailboxes};

const SERIALISE: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('hosting_mail:account:' || $1, 0))";

pub(crate) async fn domains(call: Call<Value>) -> Result<()> {
    let requested: Option<Id> = serde_json::from_value(call.params()["account"].clone()).ok();
    let Ok(account) = call.resolve_account(requested).await else {
        return Ok(());
    };
    let account = account.to_string();
    let tx = call.tx()?;
    serialise(tx, &account).await?;
    let kept = domain::Entity::find()
        .filter(domain::Column::AccountId.eq(account))
        .count(tx)
        .await?;
    call.check_limit(DOMAIN_LIMIT, kept).await
}

pub(crate) async fn mailboxes(call: Call<Value>) -> Result<()> {
    let Ok(domain) = serde_json::from_value::<Id>(call.params()["domain"].clone()) else {
        return Ok(());
    };
    let Ok(owner) = mailboxes::domain_of(&call, &domain).await else {
        return Ok(());
    };
    let Ok(account) = owner.account_id.parse::<Id>() else {
        return Ok(());
    };
    let account = call.resolve_account(Some(account)).await?.to_string();
    let tx = call.tx()?;
    serialise(tx, &account).await?;
    let kept = mailbox::Entity::find()
        .filter(mailbox::Column::AccountId.eq(account))
        .count(tx)
        .await?;
    call.check_limit(MAILBOX_LIMIT, kept).await
}

async fn serialise<C: ConnectionTrait>(db: &C, account: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SERIALISE,
        [account.into()],
    ))
    .await?;
    Ok(())
}
