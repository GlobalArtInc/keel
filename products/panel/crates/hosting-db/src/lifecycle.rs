use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::host::Effects;
use crate::{databases, login, logins};

pub(crate) fn contribution() -> AccountLifecycle {
    AccountLifecycle::new()
        .on_suspend(suspend)
        .on_resume(resume)
        .on_delete(remove)
}

async fn suspend(call: Call<Id>) -> Result<()> {
    switch(&call, login::STATE_ACTIVE, login::STATE_SUSPENDED, true).await
}

async fn resume(call: Call<Id>) -> Result<()> {
    switch(&call, login::STATE_SUSPENDED, login::STATE_ACTIVE, false).await
}

async fn switch(call: &Call<Id>, from: &str, to: &str, locked: bool) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let names: Vec<String> = login::Entity::find()
        .filter(login::Column::AccountId.eq(account.as_str()))
        .filter(login::Column::State.eq(from))
        .all(tx)
        .await?
        .into_iter()
        .map(|row| row.name)
        .collect();
    if names.is_empty() {
        return Ok(());
    }
    login::Entity::update_many()
        .col_expr(login::Column::State, Expr::value(to))
        .col_expr(login::Column::UpdatedAt, Expr::value(now))
        .filter(login::Column::AccountId.eq(account.as_str()))
        .filter(login::Column::State.eq(from))
        .exec(tx)
        .await?;
    let effects = Effects::of(call)?;
    call.after_commit(move |ctx| async move { effects.lock(&ctx, &names, locked).await });
    Ok(())
}

async fn remove(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let users = logins::drop_rows(tx, login::Column::AccountId.eq(account.as_str())).await?;
    let gone =
        databases::drop_rows(tx, crate::database::Column::AccountId.eq(account.as_str())).await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} database(s) and {} database user(s) of account {account}",
            gone.len(),
            users.len()
        ))
        .await?;
    }
    logins::drop_after_commit(&call, users)?;
    databases::drop_after_commit(&call, gone)
}
