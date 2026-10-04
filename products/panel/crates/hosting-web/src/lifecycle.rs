use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
};
use keel::sdk::system::AccountSource;
use keel::types::AccountState;

use crate::functions::{drop_rows, remove_after_commit};
use crate::host::{Effects, Turn};
use crate::limit::serialise;
use crate::{service, site};

const MISSING: &str = "keel.error.service_missing";

pub(crate) fn contribution(turn: Turn) -> AccountLifecycle {
    let suspending = Arc::clone(&turn);
    let resuming = Arc::clone(&turn);
    AccountLifecycle::new()
        .on_suspend(move |call: Call<Id>| {
            let turn = Arc::clone(&suspending);
            async move { switch(&call, &turn, *call.params()).await }
        })
        .on_resume(move |call: Call<Id>| {
            let turn = Arc::clone(&resuming);
            async move { switch(&call, &turn, *call.params()).await }
        })
        .on_delete(move |call: Call<Id>| {
            let turn = Arc::clone(&turn);
            async move { remove(&call, &turn).await }
        })
}

pub(crate) async fn off<C: ConnectionTrait>(
    tx: &C,
    accounts: &dyn AccountSource,
    db: &Db,
    account: Id,
) -> Result<bool> {
    let active = accounts
        .by_id(account, db)
        .await?
        .is_some_and(|found| found.state == AccountState::Active);
    if !active {
        return Ok(true);
    }
    let holds: Vec<bool> = service::Entity::find()
        .select_only()
        .column(service::Column::Held)
        .filter(service::Column::AccountId.eq(account.to_string()))
        .into_tuple()
        .all(tx)
        .await?;
    Ok(!holds.is_empty() && holds.into_iter().all(|held| held))
}

async fn switched<C: ConnectionTrait>(
    tx: &C,
    accounts: &dyn AccountSource,
    db: &Db,
    now: DateTime<Utc>,
    account: Id,
) -> Result<Vec<String>> {
    serialise(tx, &account.to_string()).await?;
    let suspended = off(tx, accounts, db, account).await?;
    let ids: Vec<String> = site::Entity::find()
        .select_only()
        .column(site::Column::Id)
        .filter(site::Column::AccountId.eq(account.to_string()))
        .filter(site::Column::Suspended.eq(!suspended))
        .order_by_asc(site::Column::Domain)
        .into_tuple()
        .all(tx)
        .await?;
    if !ids.is_empty() {
        site::Entity::update_many()
            .col_expr(site::Column::Suspended, Expr::value(suspended))
            .col_expr(site::Column::State, Expr::value(site::STATE_PENDING))
            .col_expr(site::Column::UpdatedAt, Expr::value(now))
            .filter(site::Column::Id.is_in(ids.clone()))
            .exec(tx)
            .await?;
    }
    Ok(ids)
}

async fn switch<P>(call: &Call<P>, turn: &Turn, account: Id) -> Result<()> {
    let tx = call.tx()?;
    let accounts = call.require::<dyn AccountSource>()?;
    let now = call.require::<dyn Clock>()?.now();
    let ids = switched(tx, accounts.as_ref(), tx, now, account).await?;
    if ids.is_empty() {
        return Ok(());
    }
    let effects = Effects::of(call, turn)?;
    call.after_commit(move |ctx| async move { effects.apply(&ctx, &ids).await });
    Ok(())
}

pub(crate) fn later<P>(call: &Call<P>, turn: &Turn, account: Id) -> Result<()> {
    let accounts = call.require::<dyn AccountSource>()?;
    let clock = call.require::<dyn Clock>()?;
    let effects = Effects::of(call, turn)?;
    call.after_commit(move |ctx| async move {
        let db = ctx.db()?.clone();
        let pool = db
            .connection()
            .ok_or_else(|| KeelError::internal(MISSING).param("service", "database"))?;
        let tx = pool.begin().await?;
        let ids = switched(&tx, accounts.as_ref(), &db, clock.now(), account).await?;
        tx.commit().await?;
        if ids.is_empty() {
            return Ok(());
        }
        effects.apply(&ctx, &ids).await
    });
    Ok(())
}

async fn remove(call: &Call<Id>, turn: &Turn) -> Result<()> {
    let account = call.params().to_string();
    let tx = call.tx()?;
    let gone = drop_rows(tx, site::Column::AccountId.eq(account.as_str())).await?;
    service::Entity::delete_many()
        .filter(service::Column::AccountId.eq(account.as_str()))
        .exec(tx)
        .await?;
    if let Some(task) = call.task() {
        task.log(&format!(
            "removed {} site(s) of account {account}",
            gone.len()
        ))
        .await?;
    }
    remove_after_commit(call, turn, gone)
}
