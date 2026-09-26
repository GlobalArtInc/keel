use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::functions::{drop_rows, remove_after_commit};
use crate::host::Effects;
use crate::user;

pub(crate) fn contribution() -> AccountLifecycle {
    AccountLifecycle::new()
        .on_suspend(suspend)
        .on_resume(resume)
        .on_delete(remove)
}

async fn suspend(call: Call<Id>) -> Result<()> {
    switch(&call, user::STATE_ACTIVE, user::STATE_SUSPENDED, true).await
}

async fn resume(call: Call<Id>) -> Result<()> {
    switch(&call, user::STATE_SUSPENDED, user::STATE_ACTIVE, false).await
}

async fn switch(call: &Call<Id>, from: &str, to: &str, locked: bool) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let names: Vec<String> = user::Entity::find()
        .filter(user::Column::AccountId.eq(account.as_str()))
        .filter(user::Column::State.eq(from))
        .all(tx)
        .await?
        .into_iter()
        .map(|row| row.name)
        .collect();
    if names.is_empty() {
        return Ok(());
    }
    user::Entity::update_many()
        .col_expr(user::Column::State, Expr::value(to))
        .col_expr(user::Column::UpdatedAt, Expr::value(now))
        .filter(user::Column::AccountId.eq(account.as_str()))
        .filter(user::Column::State.eq(from))
        .exec(tx)
        .await?;
    let effects = Effects::of(call)?;
    call.after_commit(move |ctx| async move { effects.lock(&ctx, &names, locked).await });
    Ok(())
}

async fn remove(call: Call<Id>) -> Result<()> {
    let account = call.params().to_string();
    let names = drop_rows(call.tx()?, user::Column::AccountId.eq(account.as_str())).await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} hosting user(s) of account {account}",
            names.len()
        ))
        .await?;
    }
    remove_after_commit(&call, names)
}
