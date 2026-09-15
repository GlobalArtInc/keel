use std::fmt;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use keel_sdk::KeelError;
use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Emitted;
use keel_sdk::system::OutboxSubscriber;
use keel_types::{Event, EventActor, EventActorKind, EventSubject, Id, Level, Principal};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement, Value as DbValue,
};
use serde_json::Value;
use tokio::sync::watch;

use crate::storage::lock;

pub const MAX_BATCH: u32 = 1000;

pub async fn write<C: ConnectionTrait>(
    db: &C,
    module: &str,
    principal: &Principal,
    events: Vec<Emitted>,
    now: DateTime<Utc>,
) -> Result<Option<u64>, KeelError> {
    if events.is_empty() {
        return Ok(None);
    }
    let actor = (principal.level != Level::Guest).then_some(EventActor {
        kind: EventActorKind::Principal,
        id: principal.id,
    });
    let actor_json = actor.map(serde_json::to_value).transpose()?;
    db.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::OUTBOX))
        .await?;
    let mut last = None;
    for event in events {
        let subject_json = event.subject.map(serde_json::to_value).transpose()?;
        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO keel_outbox (id, type, module, at, actor_json, subject_json, data_json, created_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING seq",
                [
                    DbValue::from(Id::generate().to_string()),
                    DbValue::from(event.kind),
                    DbValue::from(module),
                    DbValue::from(now),
                    DbValue::Json(actor_json.clone().map(Box::new)),
                    DbValue::Json(subject_json.map(Box::new)),
                    DbValue::Json(Some(Box::new(event.data))),
                    DbValue::from(now),
                ],
            ))
            .await?
            .ok_or_else(|| KeelError::internal("keel.error.outbox_seq"))?;
        let seq: i64 = row.try_get("", "seq")?;
        last = Some(u64::try_from(seq).unwrap_or_default());
    }
    Ok(last)
}

struct Attached {
    db: DatabaseConnection,
    instance: Id,
}

pub struct OutboxFeed {
    attached: Mutex<Option<Attached>>,
    latest: watch::Sender<u64>,
}

impl Default for OutboxFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl OutboxFeed {
    pub fn new() -> Self {
        let (latest, _) = watch::channel(0);
        Self {
            attached: Mutex::new(None),
            latest,
        }
    }

    pub fn fill(&self, db: DatabaseConnection, instance: Id) {
        *self.attached.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Attached { db, instance });
    }

    pub fn is_filled(&self) -> bool {
        self.attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    pub fn publish(&self, seq: u64) {
        self.latest.send_if_modified(|current| {
            if seq > *current {
                *current = seq;
                true
            } else {
                false
            }
        });
    }

    fn attached(&self) -> Result<(DatabaseConnection, Id), KeelError> {
        self.attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|attached| (attached.db.clone(), attached.instance))
            .ok_or_else(|| KeelError::unavailable("keel.error.outbox_unavailable"))
    }
}

#[async_trait]
impl OutboxSubscriber for OutboxFeed {
    async fn after(&self, seq: u64, limit: u32) -> Result<Vec<Event>, KeelError> {
        let (db, instance) = self.attached()?;
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT seq, id, type, module, at, actor_json, subject_json, data_json FROM keel_outbox WHERE seq > $1 ORDER BY seq LIMIT $2",
                [
                    DbValue::from(i64::try_from(seq).unwrap_or(i64::MAX)),
                    DbValue::from(i64::from(limit.min(MAX_BATCH))),
                ],
            ))
            .await?;
        rows.iter().map(|row| event(row, instance)).collect()
    }

    fn latest(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }
}

fn event(row: &QueryResult, instance: Id) -> Result<Event, KeelError> {
    let seq: i64 = row.try_get("", "seq")?;
    let id: String = row.try_get("", "id")?;
    let kind: String = row.try_get("", "type")?;
    let module: String = row.try_get("", "module")?;
    let at: DateTime<Utc> = row.try_get("", "at")?;
    let actor: Option<Value> = row.try_get("", "actor_json")?;
    let subject: Option<Value> = row.try_get("", "subject_json")?;
    let data: Value = row.try_get("", "data_json")?;
    Ok(Event {
        id: id.parse().map_err(|_| {
            KeelError::internal("keel.error.outbox_row")
                .param("seq", seq)
                .param("member", "id")
        })?,
        seq: u64::try_from(seq).unwrap_or_default(),
        kind,
        at,
        instance_id: instance,
        module,
        actor: actor
            .map(serde_json::from_value::<EventActor>)
            .transpose()?,
        subject: subject
            .map(serde_json::from_value::<EventSubject>)
            .transpose()?,
        data,
    })
}

impl fmt::Debug for OutboxFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutboxFeed")
            .field("filled", &self.is_filled())
            .field("latest", &*self.latest.borrow())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_feed_publishes_only_a_higher_seq_and_answers_nothing_before_it_is_filled() {
        let feed = OutboxFeed::new();
        let mut latest = feed.latest();
        assert_eq!(*latest.borrow(), 0);
        feed.publish(7);
        assert!(latest.has_changed().expect("the sender lives"));
        assert_eq!(*latest.borrow_and_update(), 7);
        feed.publish(3);
        assert!(!latest.has_changed().expect("the sender lives"));
        feed.publish(7);
        assert!(!latest.has_changed().expect("the sender lives"));
        assert!(!feed.is_filled());
        let error = feed
            .after(0, 10)
            .await
            .expect_err("an unfilled feed answers nothing");
        assert_eq!(error.key(), "keel.error.outbox_unavailable");
        assert_eq!(
            format!("{feed:?}"),
            "OutboxFeed { filled: false, latest: 7 }"
        );
    }
}
