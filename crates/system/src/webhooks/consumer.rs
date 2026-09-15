use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Set, Statement,
    TryInsertResult, Value as DbValue,
};
use keel_sdk::types::Event;

use super::entity::{cursor, delivery, subscription};
use super::masks;
use super::started::Started;

pub const CURSOR: &str = "deliveries";
pub const OWNER_ID: Id = Id::ROOT_ACCOUNT;
pub const BATCH: u32 = 200;
pub const RETRY_PAUSE: Duration = Duration::from_secs(5);
pub const STATE_PENDING: &str = "pending";
pub const STATE_DELIVERED: &str = "delivered";
pub const STATE_FAILED: &str = "failed";
pub const STATE_DEAD: &str = "dead";
pub const STATES: [&str; 4] = [STATE_PENDING, STATE_DELIVERED, STATE_FAILED, STATE_DEAD];

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
        "INSERT INTO webhook_cursor (consumer, seq) VALUES ($1, $2) ON CONFLICT (consumer) DO UPDATE SET seq = EXCLUDED.seq",
        [
            DbValue::from(CURSOR),
            DbValue::from(i64::try_from(seq).unwrap_or(i64::MAX)),
        ],
    ))
    .await?;
    Ok(())
}

pub async fn admits(scope: &AccountScope, event: &Event, db: &Db) -> Result<bool> {
    let account = event
        .subject
        .as_ref()
        .and_then(|subject| subject.account_id);
    Ok(match (scope, account) {
        (AccountScope::All, _) => true,
        (AccountScope::Nothing, _) | (_, None) => false,
        (AccountScope::Own(own), Some(account)) => *own == account,
        (AccountScope::Subtree(_), Some(account)) => scope.includes(db, account).await?,
    })
}

pub async fn enqueue(
    db: &Db,
    subscription: &subscription::Model,
    event: &Event,
    now: DateTime<Utc>,
) -> Result<Option<Id>> {
    let id = Id::generate();
    let outcome = delivery::Entity::insert(delivery::ActiveModel {
        id: Set(id.to_string()),
        subscription_id: Set(subscription.id.clone()),
        account_id: Set(subscription.account_id.clone()),
        event_id: Set(event.id.to_string()),
        seq: Set(i64::try_from(event.seq).unwrap_or(i64::MAX)),
        kind: Set(event.kind.clone()),
        attempt: Set(0),
        state: Set(STATE_PENDING.to_owned()),
        next_at: Set(now),
        last_status: Set(None),
        last_error: Set(None),
        delivered_at: Set(None),
        created_at: Set(now),
    })
    .on_conflict(
        OnConflict::columns([delivery::Column::SubscriptionId, delivery::Column::EventId])
            .do_nothing()
            .to_owned(),
    )
    .do_nothing()
    .exec(db)
    .await?;
    Ok(matches!(outcome, TryInsertResult::Inserted(_)).then_some(id))
}

struct Scopes<'a> {
    started: &'a Started,
    known: HashMap<(String, String), Option<AccountScope>>,
}

impl Scopes<'_> {
    async fn of(&mut self, subscription: &subscription::Model) -> Result<Option<AccountScope>> {
        let key = (
            subscription.principal_id.clone(),
            subscription.account_id.clone(),
        );
        if let Some(scope) = self.known.get(&key) {
            return Ok(*scope);
        }
        let scope = self.derive(subscription).await?;
        self.known.insert(key, scope);
        Ok(scope)
    }

    async fn derive(&self, subscription: &subscription::Model) -> Result<Option<AccountScope>> {
        let db = self.started.db();
        let subscriber = match subscription.principal_id.parse::<Id>() {
            Ok(id) if id == OWNER_ID => Some((Id::ROOT_ACCOUNT, AccountScope::All)),
            Ok(id) => self
                .started
                .seams
                .principals
                .by_id(id, db)
                .await?
                .map(|principal| (principal.account_id, AccountScope::of(&principal))),
            Err(_) => None,
        };
        let Some((home, scope)) = subscriber else {
            tracing::warn!(
                subscription = subscription.id,
                principal = subscription.principal_id,
                "the subscriber is gone; the subscription receives nothing"
            );
            return Ok(None);
        };
        let placed = match subscription.account_id.parse::<Id>() {
            Ok(account) if account == home => return Ok(Some(scope)),
            Ok(account) if scope.includes(db, account).await? => Some(AccountScope::Own(account)),
            Ok(_) | Err(_) => None,
        };
        if placed.is_none() {
            tracing::warn!(
                subscription = subscription.id,
                principal = subscription.principal_id,
                account = subscription.account_id,
                "the subscriber does not reach the subscription's account; the subscription receives nothing"
            );
        }
        Ok(placed)
    }
}

async fn step(started: &Started, seq: &mut u64) -> Result<usize> {
    let events = started.seams.subscriber.after(*seq, BATCH).await?;
    let Some(last) = events.last().map(|event| event.seq) else {
        return Ok(0);
    };
    let subscriptions = subscription::Entity::find()
        .filter(subscription::Column::Enabled.eq(true))
        .all(started.db())
        .await?;
    let now = started.seams.clock.now();
    let mut scopes = Scopes {
        started,
        known: HashMap::new(),
    };
    let mut created = 0;
    for event in &events {
        for subscription in &subscriptions {
            if !masks::any_matches(subscription.events.iter().map(String::as_str), &event.kind) {
                continue;
            }
            let Some(scope) = scopes.of(subscription).await? else {
                continue;
            };
            if !admits(&scope, event, started.db()).await? {
                continue;
            }
            if enqueue(started.db(), subscription, event, now)
                .await?
                .is_some()
            {
                created += 1;
            }
        }
    }
    store_cursor(started.db(), last).await?;
    *seq = last;
    Ok(created)
}

pub async fn run(started: Arc<Started>) {
    let mut seq = match cursor(started.db()).await {
        Ok(seq) => seq,
        Err(error) => {
            tracing::error!(error = %error, "the deliveries cursor could not be read; starting from the beginning");
            0
        }
    };
    let mut latest = started.seams.subscriber.latest();
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
                tracing::error!(error = %error, seq, "the deliveries consumer failed; retrying");
                tokio::time::sleep(RETRY_PAUSE).await;
                continue;
            }
        }
        if latest.changed().await.is_err() {
            tracing::info!("the outbox feed closed; the deliveries consumer stops");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::sea_orm::DatabaseConnection;
    use keel_sdk::types::EventSubject;

    use super::*;

    fn event(account: Option<Id>) -> Event {
        Event {
            id: Id::generate(),
            seq: 1,
            kind: "note.saved".into(),
            at: Utc::now(),
            instance_id: Id::generate(),
            module: "note".into(),
            actor: None,
            subject: account.map(|account| EventSubject {
                kind: "note".into(),
                id: Id::generate(),
                account_id: Some(account),
            }),
            data: json!({}),
        }
    }

    #[tokio::test]
    async fn an_event_is_admitted_to_an_admin_always_and_to_a_user_for_the_own_account_alone() {
        let db = Db::from_connection(DatabaseConnection::Disconnected);
        let a1 = Id::generate();
        let a2 = Id::generate();
        assert!(
            admits(&AccountScope::All, &event(Some(a1)), &db)
                .await
                .unwrap()
        );
        assert!(admits(&AccountScope::All, &event(None), &db).await.unwrap());
        assert!(
            admits(&AccountScope::Own(a1), &event(Some(a1)), &db)
                .await
                .unwrap()
        );
        assert!(
            !admits(&AccountScope::Own(a1), &event(Some(a2)), &db)
                .await
                .unwrap()
        );
        assert!(
            !admits(&AccountScope::Own(a1), &event(None), &db)
                .await
                .unwrap()
        );
        assert!(
            !admits(&AccountScope::Subtree(a1), &event(None), &db)
                .await
                .unwrap()
        );
        assert!(
            !admits(&AccountScope::Nothing, &event(Some(a1)), &db)
                .await
                .unwrap()
        );
        assert_eq!(STATES, ["pending", "delivered", "failed", "dead"]);
    }
}
