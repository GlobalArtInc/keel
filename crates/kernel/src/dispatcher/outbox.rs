use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Emitted;
use keel_sdk::system::{EventRelay, OutboxSubscriber};
use keel_sdk::{Clock, KeelError};
use keel_types::{Event, EventActor, EventActorKind, EventSubject, Id, Level, Principal, Via};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement, TransactionTrait,
    Value as DbValue,
};
use serde_json::Value;
use tokio::sync::watch;

use crate::storage::lock;

pub const MAX_BATCH: u32 = 1000;
pub const RELAY_LOCAL_KEY: &str = "keel.error.relay_local";
pub const RELAY_TAKEN_KEY: &str = "keel.error.relay_taken";

pub fn actor_of(principal: &Principal) -> Option<EventActor> {
    if principal.level == Level::Guest {
        return None;
    }
    Some(match principal.via {
        Via::Peer(_) => EventActor {
            kind: EventActorKind::Peer,
            id: principal
                .actor
                .as_ref()
                .map_or(principal.id, |actor| actor.id),
        },
        _ => EventActor {
            kind: EventActorKind::Principal,
            id: principal.id,
        },
    })
}

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
    let actor_json = actor_of(principal).map(serde_json::to_value).transpose()?;
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
    clock: Arc<dyn Clock>,
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

    pub fn fill(&self, db: DatabaseConnection, instance: Id, clock: Arc<dyn Clock>) {
        *self.attached.lock().unwrap_or_else(PoisonError::into_inner) = Some(Attached {
            db,
            instance,
            clock,
        });
    }

    pub fn is_filled(&self) -> bool {
        self.attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    pub fn instance(&self) -> Result<Id, KeelError> {
        Ok(self.attached()?.instance)
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

    pub async fn newest(&self) -> Result<u64, KeelError> {
        let Attached { db, .. } = self.attached()?;
        let row = db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT COALESCE(MAX(seq), 0) AS seq FROM keel_outbox",
            ))
            .await?;
        let seq: i64 = match row {
            Some(row) => row.try_get("", "seq")?,
            None => 0,
        };
        let seq = u64::try_from(seq).unwrap_or_default();
        self.publish(seq);
        Ok(seq)
    }

    fn attached(&self) -> Result<Attached, KeelError> {
        self.attached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|attached| Attached {
                db: attached.db.clone(),
                instance: attached.instance,
                clock: attached.clock.clone(),
            })
            .ok_or_else(|| KeelError::unavailable("keel.error.outbox_unavailable"))
    }
}

#[async_trait]
impl OutboxSubscriber for OutboxFeed {
    async fn after(&self, seq: u64, limit: u32) -> Result<Vec<Event>, KeelError> {
        let Attached { db, instance, .. } = self.attached()?;
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT seq, id, type, module, at, actor_json, subject_json, data_json, instance_id FROM keel_outbox WHERE seq > $1 ORDER BY seq LIMIT $2",
                [
                    DbValue::from(i64::try_from(seq).unwrap_or(i64::MAX)),
                    DbValue::from(i64::from(limit.min(MAX_BATCH))),
                ],
            ))
            .await?;
        rows.iter().map(|row| event(row, instance)).collect()
    }

    async fn newest(&self) -> Result<u64, KeelError> {
        OutboxFeed::newest(self).await
    }

    fn latest(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }
}

#[async_trait]
impl EventRelay for OutboxFeed {
    async fn relay(&self, event: &Event) -> Result<Option<u64>, KeelError> {
        let Attached {
            db,
            instance,
            clock,
        } = self.attached()?;
        if event.instance_id == instance {
            return Err(KeelError::validation(RELAY_LOCAL_KEY)
                .param("id", event.id.to_string())
                .param("instanceId", instance.to_string()));
        }
        let actor_json = event.actor.as_ref().map(serde_json::to_value).transpose()?;
        let subject_json = event
            .subject
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?;
        let tx = db.begin().await?;
        tx.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::OUTBOX))
            .await?;
        let row = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO keel_outbox (id, type, module, at, actor_json, subject_json, data_json, created_at, instance_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (id) DO NOTHING RETURNING seq",
                [
                    DbValue::from(event.id.to_string()),
                    DbValue::from(event.kind.as_str()),
                    DbValue::from(event.module.as_str()),
                    DbValue::from(event.at),
                    DbValue::Json(actor_json.map(Box::new)),
                    DbValue::Json(subject_json.map(Box::new)),
                    DbValue::Json(Some(Box::new(event.data.clone()))),
                    DbValue::from(clock.now()),
                    DbValue::from(event.instance_id.to_string()),
                ],
            ))
            .await?;
        let Some(row) = row else {
            let holder = tx
                .query_one(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT instance_id FROM keel_outbox WHERE id = $1",
                    [DbValue::from(event.id.to_string())],
                ))
                .await?;
            tx.commit().await?;
            let holder = match holder {
                Some(row) => row
                    .try_get::<Option<String>>("", "instance_id")?
                    .unwrap_or_else(|| instance.to_string()),
                None => return Ok(None),
            };
            if holder == event.instance_id.to_string() {
                return Ok(None);
            }
            return Err(KeelError::validation(RELAY_TAKEN_KEY)
                .param("id", event.id.to_string())
                .param("instanceId", holder));
        };
        tx.commit().await?;
        let seq: i64 = row.try_get("", "seq")?;
        let seq = u64::try_from(seq).unwrap_or_default();
        self.publish(seq);
        Ok(Some(seq))
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
    let origin: Option<String> = row.try_get("", "instance_id")?;
    let instance_id = match origin {
        Some(origin) => origin.parse().map_err(|_| {
            KeelError::internal("keel.error.outbox_row")
                .param("seq", seq)
                .param("member", "instance_id")
        })?,
        None => instance,
    };
    Ok(Event {
        id: id.parse().map_err(|_| {
            KeelError::internal("keel.error.outbox_row")
                .param("seq", seq)
                .param("member", "id")
        })?,
        seq: u64::try_from(seq).unwrap_or_default(),
        kind,
        at,
        instance_id,
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
    use std::collections::BTreeMap;

    use keel_types::Scope;

    use super::*;

    fn principal(level: Level, via: Via) -> Principal {
        Principal {
            id: Id::generate(),
            login: "someone".into(),
            level,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: BTreeMap::new(),
        }
    }

    #[test]
    fn the_actor_is_the_principal_the_peer_link_for_a_call_through_it_and_none_for_a_guest() {
        let local = principal(Level::User, Via::Http);
        assert_eq!(
            actor_of(&local),
            Some(EventActor {
                kind: EventActorKind::Principal,
                id: local.id,
            })
        );
        let link = principal(Level::Admin, Via::Peer(Id::generate()));
        assert_eq!(
            actor_of(&link),
            Some(EventActor {
                kind: EventActorKind::Peer,
                id: link.id,
            })
        );
        let mut customer = principal(Level::User, link.via.clone());
        customer.actor = Some(Box::new(link.clone()));
        assert_eq!(
            actor_of(&customer),
            Some(EventActor {
                kind: EventActorKind::Peer,
                id: link.id,
            })
        );
        let mut su = principal(Level::User, Via::Su);
        su.actor = Some(Box::new(local.clone()));
        assert_eq!(
            actor_of(&su).map(|actor| actor.kind),
            Some(EventActorKind::Principal)
        );
        assert_eq!(actor_of(&su).map(|actor| actor.id), Some(su.id));
        assert_eq!(actor_of(&principal(Level::Guest, Via::Http)), None);
    }

    #[tokio::test]
    async fn an_unfilled_feed_relays_nothing() {
        let feed = OutboxFeed::new();
        let event = Event {
            id: Id::generate(),
            seq: 1,
            kind: "dns.zone.created".into(),
            at: Utc::now(),
            instance_id: Id::generate(),
            module: "dns".into(),
            actor: None,
            subject: None,
            data: Value::Null,
        };
        let error = feed
            .relay(&event)
            .await
            .expect_err("an unfilled feed has no database");
        assert_eq!(error.key(), "keel.error.outbox_unavailable");
    }

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
        let error = feed
            .instance()
            .expect_err("an unfilled feed names no installation");
        assert_eq!(error.key(), "keel.error.outbox_unavailable");
        assert_eq!(
            format!("{feed:?}"),
            "OutboxFeed { filled: false, latest: 7 }"
        );
    }
}
