use std::sync::Arc;
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::{Expr, OnConflict, Query};
use keel_sdk::sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, Statement,
};
use keel_sdk::serde_json;
use keel_sdk::types::{ErrorCode, Event};

use super::delivery::{Delivery, RETRY_PAUSE, Seams};
use super::entity::{inbox, product};
use super::identity;

pub const BATCH: u64 = 100;
pub const POLL: Duration = Duration::from_secs(5);
pub const RETENTION_DAYS: i64 = 7;
pub const MAX_ATTEMPTS: i32 = 5;
pub const PENDING_MAX: u64 = 10_000;
const PENDING: &str = "SELECT count(*) AS held FROM (SELECT 1 FROM peer_inbox WHERE peer_id = $1 AND processed_at IS NULL LIMIT $2) pending";

pub async fn receive(db: &Db, link: &str, event: &Event, now: DateTime<Utc>) -> Result<bool> {
    let inserted = inbox::Entity::insert(inbox::ActiveModel {
        peer_id: Set(link.to_owned()),
        event_id: Set(event.id.to_string()),
        event_json: Set(serde_json::to_value(event)?),
        received_at: Set(now),
        processed_at: Set(None),
        attempts: Set(0),
    })
    .on_conflict(
        OnConflict::columns([inbox::Column::PeerId, inbox::Column::EventId])
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(db)
    .await?;
    Ok(inserted > 0)
}

pub async fn pending(db: &Db, link: &str) -> Result<u64> {
    let held = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            PENDING,
            [
                link.into(),
                i64::try_from(PENDING_MAX).unwrap_or(i64::MAX).into(),
            ],
        ))
        .await?
        .map(|row| row.try_get::<i64>("", "held"))
        .transpose()?
        .unwrap_or_default();
    Ok(u64::try_from(held).unwrap_or_default())
}

fn linked() -> keel_sdk::sea_orm::sea_query::SelectStatement {
    Query::select()
        .column(product::Column::Id)
        .from(product::Entity)
        .to_owned()
}

pub async fn prune(db: &Db, now: DateTime<Utc>) -> Result<u64> {
    let before = now - keel_sdk::chrono::Duration::days(RETENTION_DAYS);
    let pruned = inbox::Entity::delete_many()
        .filter(
            Condition::any()
                .add(inbox::Column::ProcessedAt.lte(before))
                .add(inbox::Column::PeerId.not_in_subquery(linked())),
        )
        .exec(db)
        .await?;
    Ok(pruned.rows_affected)
}

pub async fn forget(db: &Db, link: &str) -> Result<u64> {
    let forgotten = inbox::Entity::delete_many()
        .filter(inbox::Column::PeerId.eq(link))
        .exec(db)
        .await?;
    Ok(forgotten.rows_affected)
}

fn of_row(row: &inbox::Model) -> Condition {
    Condition::all()
        .add(inbox::Column::PeerId.eq(row.peer_id.as_str()))
        .add(inbox::Column::EventId.eq(row.event_id.as_str()))
}

async fn settle(db: &Db, row: &inbox::Model, now: DateTime<Utc>) -> Result<()> {
    inbox::Entity::update_many()
        .col_expr(inbox::Column::ProcessedAt, Expr::value(Some(now)))
        .filter(of_row(row))
        .exec(db)
        .await?;
    Ok(())
}

async fn postpone(db: &Db, row: &inbox::Model) -> Result<()> {
    inbox::Entity::update_many()
        .col_expr(
            inbox::Column::Attempts,
            Expr::col(inbox::Column::Attempts).add(1),
        )
        .filter(of_row(row))
        .exec(db)
        .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Relayed,
    Dropped,
    Postponed,
}

fn retried(row: &inbox::Model, error: &KeelError, what: &str) -> Outcome {
    if row.attempts.saturating_add(1) >= MAX_ATTEMPTS {
        tracing::error!(event = row.event_id, peer = row.peer_id, attempts = MAX_ATTEMPTS, error = %error, "an event of a peer did not {what} after its last attempt; it is dropped");
        Outcome::Dropped
    } else {
        tracing::warn!(event = row.event_id, peer = row.peer_id, attempt = row.attempts.saturating_add(1), error = %error, "an event of a peer did not {what}; it is tried again");
        Outcome::Postponed
    }
}

async fn relay(seams: &Seams, row: &inbox::Model, event: &Event) -> Outcome {
    match seams.relay.relay(event).await {
        Ok(_) => Outcome::Relayed,
        Err(error) if error.code() == ErrorCode::Validation => {
            tracing::error!(event = row.event_id, peer = row.peer_id, error = %error, "an event of a peer cannot enter the bus; it is dropped");
            Outcome::Dropped
        }
        Err(error) => retried(row, &error, "enter the bus"),
    }
}

async fn process(seams: &Seams, row: &inbox::Model, identities: &[product::Model]) -> Outcome {
    let event = match serde_json::from_value::<Event>(row.event_json.clone()) {
        Ok(event) => event,
        Err(error) => {
            tracing::error!(event = row.event_id, peer = row.peer_id, error = %error, "a stored event of a peer cannot be read; it is dropped");
            return Outcome::Dropped;
        }
    };
    let relayed = relay(seams, row, &event).await;
    if relayed != Outcome::Relayed {
        return relayed;
    }
    let Some(link) = identities.iter().find(|link| link.id == row.peer_id) else {
        return relayed;
    };
    match identity::apply(seams, link, &event).await {
        Ok(()) => Outcome::Relayed,
        Err(error) => retried(row, &error, "apply to the identity it changes"),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Drained {
    count: usize,
    postponed: usize,
}

async fn drain(delivery: &Delivery) -> Result<Drained> {
    let seams = delivery.seams()?;
    let identities = product::Entity::find()
        .filter(product::Column::Identity.eq(true))
        .all(&seams.db)
        .await?;
    let rows = inbox::Entity::find()
        .filter(inbox::Column::ProcessedAt.is_null())
        .filter(inbox::Column::PeerId.in_subquery(linked()))
        .order_by_asc(inbox::Column::ReceivedAt)
        .order_by_asc(inbox::Column::PeerId)
        .order_by_asc(inbox::Column::EventId)
        .limit(BATCH)
        .all(&seams.db)
        .await?;
    let mut drained = Drained {
        count: rows.len(),
        postponed: 0,
    };
    for row in rows {
        match process(seams, &row, &identities).await {
            Outcome::Postponed => {
                postpone(&seams.db, &row).await?;
                drained.postponed += 1;
            }
            Outcome::Relayed | Outcome::Dropped => {
                settle(&seams.db, &row, seams.clock.now()).await?;
            }
        }
    }
    Ok(drained)
}

pub async fn run(delivery: Arc<Delivery>) {
    loop {
        tokio::select! {
            () = delivery.woken() => {}
            () = tokio::time::sleep(POLL) => {}
        }
        loop {
            match drain(&delivery).await {
                Ok(drained) if drained.postponed > 0 => tokio::time::sleep(RETRY_PAUSE).await,
                Ok(drained) if drained.count as u64 >= BATCH => {}
                Ok(_) => break,
                Err(error) => {
                    tracing::error!(error = %error, "the peer inbox could not be processed; retrying");
                    tokio::time::sleep(RETRY_PAUSE).await;
                }
            }
        }
    }
}
