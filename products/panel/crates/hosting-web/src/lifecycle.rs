use keel::prelude::*;
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use crate::functions::{drop_rows, remove_after_commit};
use crate::host::{Effects, Turn};
use crate::site;

pub(crate) fn contribution(turn: Turn) -> AccountLifecycle {
    let suspending = Arc::clone(&turn);
    let resuming = Arc::clone(&turn);
    AccountLifecycle::new()
        .on_suspend(move |call: Call<Id>| {
            let turn = Arc::clone(&suspending);
            async move { switch(&call, &turn, true).await }
        })
        .on_resume(move |call: Call<Id>| {
            let turn = Arc::clone(&resuming);
            async move { switch(&call, &turn, false).await }
        })
        .on_delete(move |call: Call<Id>| {
            let turn = Arc::clone(&turn);
            async move { remove(&call, &turn).await }
        })
}

async fn switch(call: &Call<Id>, turn: &Turn, suspended: bool) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let now = call.require::<dyn Clock>()?.now();
    let ids: Vec<String> = site::Entity::find()
        .filter(site::Column::AccountId.eq(account.as_str()))
        .filter(site::Column::Suspended.eq(!suspended))
        .all(tx)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    site::Entity::update_many()
        .col_expr(site::Column::Suspended, Expr::value(suspended))
        .col_expr(site::Column::State, Expr::value(site::STATE_PENDING))
        .col_expr(site::Column::UpdatedAt, Expr::value(now))
        .filter(site::Column::Id.is_in(ids.clone()))
        .exec(tx)
        .await?;
    let effects = Effects::of(call, turn)?;
    call.after_commit(move |ctx| async move { effects.apply(&ctx, &ids).await });
    Ok(())
}

async fn remove(call: &Call<Id>, turn: &Turn) -> Result<()> {
    let account = call.params().to_string();
    let gone = drop_rows(call.tx()?, site::Column::AccountId.eq(account.as_str())).await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} site(s) of account {account}",
            gone.len()
        ))
        .await?;
    }
    remove_after_commit(call, turn, gone)
}
