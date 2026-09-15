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

use super::api::{EVENT_MESSAGE_CREATED, SOURCE_PROBLEM};
use super::entity::{channel, cursor, delivery};
use super::mailer::{Mailer, Started};

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

pub fn wants_delivery(event: &Event) -> Option<(String, String)> {
    if event.kind != EVENT_MESSAGE_CREATED {
        return None;
    }
    let source = event.data.get("source").and_then(Value::as_str)?;
    if source != SOURCE_PROBLEM {
        return None;
    }
    let id = event.data.get("id").and_then(Value::as_str)?;
    let key = event.data.get("key").and_then(Value::as_str)?;
    Some((id.to_owned(), key.to_owned()))
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

async fn step(started: &Started, seq: &mut u64) -> Result<usize> {
    let events = started.subscriber.after(*seq, BATCH).await?;
    let Some(last) = events.last().map(|event| event.seq) else {
        return Ok(0);
    };
    let wanted: Vec<(String, String)> = events.iter().filter_map(wants_delivery).collect();
    let mut created = 0;
    if !wanted.is_empty() {
        let channels = channel::Entity::find()
            .filter(channel::Column::Enabled.eq(true))
            .all(&started.db)
            .await?;
        let now = started.clock.now();
        for (message, key) in &wanted {
            created += enqueue(&started.db, &channels, message, key, now)
                .await?
                .len();
        }
    }
    store_cursor(&started.db, last).await?;
    *seq = last;
    Ok(created)
}

pub async fn run(started: Arc<Started>, mailer: Arc<Mailer>) {
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
                    mailer.wake();
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
    fn a_problem_message_event_wants_a_delivery_and_other_events_do_not() {
        let wanted = wants_delivery(&event(
            EVENT_MESSAGE_CREATED,
            json!({ "id": "01J", "key": "notify.problem.raised", "source": "problem" }),
        ));
        assert_eq!(
            wanted,
            Some(("01J".to_owned(), "notify.problem.raised".to_owned()))
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
}
