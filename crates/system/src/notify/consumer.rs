use std::sync::Arc;
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set, Statement,
    Value as DbValue,
};
use keel_sdk::types::Event;

use super::api::{EVENT_MESSAGE_CREATED, SOURCE_PROBLEM, SOURCE_TEST};
use super::entity::{channel, cursor, delivery};
use super::started::Started;

pub const CURSOR: &str = "channels";
pub const BATCH: u32 = 200;
pub const RETRY_PAUSE: Duration = Duration::from_secs(5);
pub const STATE_PENDING: &str = "pending";
pub const STATE_SENT: &str = "sent";
pub const STATE_FAILED: &str = "failed";

pub async fn cursor(db: &Db) -> Result<u64> {
    Ok(cursor::Entity::find_by_id(CURSOR.to_owned())
        .one(db)
        .await?
        .map(|row| u64::try_from(row.seq).unwrap_or_default())
        .unwrap_or_default())
}

async fn store_cursor(db: &Db, seq: u64) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO notify_cursor (consumer, seq) VALUES ($1, $2) ON CONFLICT (consumer) DO UPDATE SET seq = EXCLUDED.seq",
        [
            DbValue::from(CURSOR),
            DbValue::from(i64::try_from(seq).unwrap_or(i64::MAX)),
        ],
    ))
    .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    EveryEnabled,
    Channel(Id),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub message: String,
    pub key: String,
    pub target: Target,
}

pub fn emitted_here(event: &Event, instance: Option<Id>) -> bool {
    instance.is_none_or(|local| local == event.instance_id)
}

pub fn wants_delivery(event: &Event) -> Option<Wanted> {
    if event.kind != EVENT_MESSAGE_CREATED {
        return None;
    }
    let field = |name: &str| event.data.get(name).and_then(Value::as_str);
    let target = match field("source")? {
        SOURCE_PROBLEM => Target::EveryEnabled,
        SOURCE_TEST => Target::Channel(field("channel")?.parse().ok()?),
        _ => return None,
    };
    Some(Wanted {
        message: field("id")?.to_owned(),
        key: field("key")?.to_owned(),
        target,
    })
}

pub async fn enqueue(
    db: &Db,
    channels: &[channel::Model],
    message: &str,
    key: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Id>> {
    let mut created = Vec::new();
    for channel in channels {
        let id = Id::generate();
        let outcome = delivery::Entity::insert(delivery::ActiveModel {
            id: Set(id.to_string()),
            channel_id: Set(channel.id.clone()),
            channel: Set(channel.name.clone()),
            message_id: Set(message.to_owned()),
            message_key: Set(key.to_owned()),
            attempt: Set(0),
            state: Set(STATE_PENDING.to_owned()),
            next_at: Set(now),
            error: Set(None),
            sent_at: Set(None),
            created_at: Set(now),
        })
        .on_conflict(
            OnConflict::columns([delivery::Column::ChannelId, delivery::Column::MessageId])
                .do_nothing()
                .to_owned(),
        )
        .do_nothing()
        .exec(db)
        .await?;
        if matches!(outcome, keel_sdk::sea_orm::TryInsertResult::Inserted(_)) {
            created.push(id);
        }
    }
    Ok(created)
}

async fn targets(started: &Started, wanted: &Wanted) -> Result<Vec<channel::Model>> {
    match &wanted.target {
        Target::EveryEnabled => Ok(channel::Entity::find()
            .filter(channel::Column::Enabled.eq(true))
            .all(&started.db)
            .await?),
        Target::Channel(id) => {
            let found = channel::Entity::find_by_id(id.to_string())
                .one(&started.db)
                .await?;
            if found.is_none() {
                tracing::warn!(message = wanted.message, channel = %id, "the tested channel is gone; no delivery");
            }
            Ok(found.into_iter().collect())
        }
    }
}

async fn step(started: &Started, seq: &mut u64) -> Result<usize> {
    let events = started.subscriber.after(*seq, BATCH).await?;
    let Some(last) = events.last().map(|event| event.seq) else {
        return Ok(0);
    };
    let now = started.clock.now();
    let mut created = 0;
    for wanted in events
        .iter()
        .filter(|event| emitted_here(event, started.instance))
        .filter_map(wants_delivery)
    {
        let channels = targets(started, &wanted).await?;
        created += enqueue(&started.db, &channels, &wanted.message, &wanted.key, now)
            .await?
            .len();
    }
    store_cursor(&started.db, last).await?;
    *seq = last;
    Ok(created)
}

pub async fn run(started: Arc<Started>) {
    let mut seq = match cursor(&started.db).await {
        Ok(seq) => seq,
        Err(error) => {
            tracing::error!(error = %error, "the channels cursor could not be read; starting from the beginning");
            0
        }
    };
    let mut latest = started.subscriber.latest();
    loop {
        match step(&started, &mut seq).await {
            Ok(created) => {
                if created > 0 {
                    started.wake();
                }
                if latest.borrow_and_update().to_owned() > seq {
                    continue;
                }
            }
            Err(error) => {
                tracing::error!(error = %error, seq, "the channels consumer failed; retrying");
                tokio::time::sleep(RETRY_PAUSE).await;
                continue;
            }
        }
        if latest.changed().await.is_err() {
            tracing::info!("the outbox feed closed; the channels consumer stops");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::types::Event;

    use super::*;

    fn event(kind: &str, data: Value) -> Event {
        Event {
            id: Id::generate(),
            seq: 1,
            kind: kind.to_owned(),
            at: Utc::now(),
            instance_id: Id::generate(),
            module: "notify".into(),
            actor: None,
            subject: None,
            data,
        }
    }

    #[test]
    fn a_problem_message_wants_every_enabled_channel_a_test_message_its_own_and_other_events_none()
    {
        let wanted = wants_delivery(&event(
            EVENT_MESSAGE_CREATED,
            json!({ "id": "01J", "key": "notify.problem.raised", "source": "problem" }),
        ));
        assert_eq!(
            wanted,
            Some(Wanted {
                message: "01J".to_owned(),
                key: "notify.problem.raised".to_owned(),
                target: Target::EveryEnabled,
            })
        );
        let channel = Id::generate();
        assert_eq!(
            wants_delivery(&event(
                EVENT_MESSAGE_CREATED,
                json!({ "id": "01J", "key": "notify.test", "source": "test", "channel": channel.to_string() }),
            )),
            Some(Wanted {
                message: "01J".to_owned(),
                key: "notify.test".to_owned(),
                target: Target::Channel(channel),
            })
        );
        assert_eq!(
            wants_delivery(&event(
                EVENT_MESSAGE_CREATED,
                json!({ "id": "01J", "key": "notify.test", "source": "test" }),
            )),
            None
        );
        assert_eq!(
            wants_delivery(&event(
                EVENT_MESSAGE_CREATED,
                json!({ "id": "01J", "key": "notify.test", "source": "test", "channel": "ops" }),
            )),
            None
        );
        assert_eq!(
            wants_delivery(&event(
                EVENT_MESSAGE_CREATED,
                json!({ "id": "01J", "key": "probe.hello", "source": "probe" }),
            )),
            None
        );
        assert_eq!(
            wants_delivery(&event(
                "notify.problem.raised",
                json!({ "id": "01J", "key": "x", "source": "problem" }),
            )),
            None
        );
        assert_eq!(
            wants_delivery(&event(
                EVENT_MESSAGE_CREATED,
                json!({ "source": "problem" })
            )),
            None
        );
    }

    #[test]
    fn an_event_relayed_from_another_installation_is_not_emitted_here() {
        let relayed = event(
            EVENT_MESSAGE_CREATED,
            json!({ "id": "01J", "key": "notify.problem.raised", "source": "problem" }),
        );
        assert!(!emitted_here(&relayed, Some(Id::generate())));
        assert!(emitted_here(&relayed, Some(relayed.instance_id)));
        assert!(emitted_here(&relayed, None));
    }
}
