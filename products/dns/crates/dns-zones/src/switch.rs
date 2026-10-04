use keel::prelude::*;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::sea_orm::sea_query::Expr;
use keel::sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Statement, TransactionTrait,
};
use keel::sdk::system::AccountSource;
use keel::types::AccountState;

use crate::provider::DnsProvider;
use crate::{service, zone};

const SERIALISE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('dns_zone:' || $1, 0))";
const MISSING: &str = "keel.error.service_missing";

pub(crate) struct Switched {
    off: bool,
    ids: Vec<String>,
}

impl Switched {
    async fn publish(self, dns: &dyn DnsProvider, ctx: &AfterContext) -> Result<()> {
        if self.ids.is_empty() {
            return Ok(());
        }
        if self.off {
            return dns.reindex(ctx).await;
        }
        let mut failed = None;
        for id in &self.ids {
            if let Err(error) = dns.apply(ctx, id).await {
                failed.get_or_insert(error);
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

pub(crate) async fn serialise<C: ConnectionTrait>(tx: &C, account: &str) -> Result<()> {
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        SERIALISE,
        [account.into()],
    ))
    .await?;
    Ok(())
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
) -> Result<Switched> {
    serialise(tx, &account.to_string()).await?;
    let off = off(tx, accounts, db, account).await?;
    let ids: Vec<String> = zone::Entity::find()
        .select_only()
        .column(zone::Column::Id)
        .filter(zone::Column::AccountId.eq(account.to_string()))
        .filter(zone::Column::Suspended.eq(!off))
        .order_by_asc(zone::Column::Name)
        .into_tuple()
        .all(tx)
        .await?;
    if !ids.is_empty() {
        let mut update = zone::Entity::update_many()
            .col_expr(zone::Column::Suspended, Expr::value(off))
            .col_expr(zone::Column::UpdatedAt, Expr::value(now));
        if !off {
            update = update.col_expr(zone::Column::State, Expr::value(zone::STATE_PENDING));
        }
        update
            .filter(zone::Column::Id.is_in(ids.clone()))
            .exec(tx)
            .await?;
    }
    Ok(Switched { off, ids })
}

pub(crate) async fn switch<P>(call: &Call<P>, account: Id) -> Result<()> {
    let tx = call.tx()?;
    let accounts = call.require::<dyn AccountSource>()?;
    let now = call.require::<dyn Clock>()?.now();
    let switched = switched(tx, accounts.as_ref(), tx, now, account).await?;
    let dns = call.require::<dyn DnsProvider>()?;
    call.after_commit(move |ctx| async move { switched.publish(dns.as_ref(), &ctx).await });
    Ok(())
}

pub(crate) fn later<P>(call: &Call<P>, account: Id) -> Result<()> {
    let accounts = call.require::<dyn AccountSource>()?;
    let clock = call.require::<dyn Clock>()?;
    let dns = call.require::<dyn DnsProvider>()?;
    call.after_commit(move |ctx| async move {
        let db = ctx.db()?.clone();
        let pool = db
            .connection()
            .ok_or_else(|| KeelError::internal(MISSING).param("service", "database"))?;
        let tx = pool.begin().await?;
        let switched = switched(&tx, accounts.as_ref(), &db, clock.now(), account).await?;
        tx.commit().await?;
        switched.publish(dns.as_ref(), &ctx).await
    });
    Ok(())
}
