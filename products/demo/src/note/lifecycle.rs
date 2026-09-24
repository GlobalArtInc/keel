use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::{Alias, Query};
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement,
};
use keel::sdk::serde_json;

use super::{Batch, item};

const HOLD: &str = "INSERT INTO note_hold (account_id, held_at) VALUES ($1, $2) ON CONFLICT (account_id) DO NOTHING";
const RELEASE: &str = "DELETE FROM note_hold WHERE account_id = $1";
const HOLD_TABLE: &str = "note_hold";
const HOLD_ACCOUNT: &str = "account_id";

pub(super) fn contribution() -> AccountLifecycle {
    AccountLifecycle::new()
        .on_suspend(hold)
        .on_resume(release)
        .on_delete(drop_notes)
}

async fn hold(call: Call<Id>) -> Result<()> {
    let now = call.require::<dyn Clock>()?.now();
    call.tx()?
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            HOLD,
            [call.params().to_string().into(), now.into()],
        ))
        .await?;
    Ok(())
}

async fn release(call: Call<Id>) -> Result<()> {
    unhold(call.tx()?, &call.params().to_string()).await
}

async fn unhold(db: &Db, account: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        RELEASE,
        [account.into()],
    ))
    .await?;
    Ok(())
}

async fn drop_notes(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let dropped = item::Entity::delete_many()
        .filter(item::Column::AccountId.eq(account.as_str()))
        .exec(tx)
        .await?
        .rows_affected;
    unhold(tx, &account).await?;
    if let Some(task) = call.task() {
        task.log(&format!("dropped {dropped} note(s) of account {account}"))
            .await?;
    }
    Ok(())
}

pub(super) async fn refuse_held(call: Call<Value>) -> Result<()> {
    let batch: Batch = serde_json::from_value(call.params().clone())?;
    let ids: Vec<String> = batch.ids.iter().map(ToString::to_string).collect();
    let held = Query::select()
        .column(Alias::new(HOLD_ACCOUNT))
        .from(Alias::new(HOLD_TABLE))
        .to_owned();
    let row = item::Entity::find()
        .filter(item::Column::Id.is_in(ids))
        .filter(item::Column::AccountId.in_subquery(held))
        .one(call.tx()?)
        .await?;
    match row {
        Some(row) => Err(KeelError::conflict("note.error.account_suspended")
            .param("state", "suspended")
            .param("title", row.title)),
        None => Ok(()),
    }
}
