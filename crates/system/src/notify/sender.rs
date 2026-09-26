use std::sync::Arc;
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use keel_sdk::system::Caller;

use super::api::{Problems, Raise, Severity};
use super::channels::email::{self, Mail};
use super::channels::{ChannelKind, ERROR_KIND_UNAVAILABLE, StoredEmail, email_of, kind_of};
use super::consumer::{STATE_FAILED, STATE_PENDING, STATE_SENT};
use super::direct;
use super::entity::{channel, delivery, message};
use super::started::Started;

pub const POLL: Duration = Duration::from_secs(1);
pub const BATCH: u64 = 50;
pub const LADDER: [Duration; 5] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 60 * 60),
    Duration::from_secs(12 * 60 * 60),
];
pub const MAX_ATTEMPTS: i32 = LADDER.len() as i32 + 1;
pub const PROBLEM_KEY: &str = "notify.delivery_failed";
pub const PROBLEM_SOURCE: &str = "notify.delivery";
pub const ERROR_CHANNEL_GONE: &str = "notify.error.channel_gone";
pub const ERROR_MESSAGE_GONE: &str = "notify.error.message_gone";

pub fn next_attempt_at(attempt: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let index = usize::try_from(attempt.saturating_sub(1)).unwrap_or_default();
    let wait = LADDER.get(index).copied()?;
    let wait = keel_sdk::chrono::Duration::from_std(wait).ok()?;
    Some(now + wait)
}

pub fn reason_of(error: &KeelError) -> String {
    error
        .params()
        .get("reason")
        .and_then(Value::as_str)
        .map(|reason| format!("{}: {reason}", error.key()))
        .unwrap_or_else(|| error.to_string())
}

pub async fn run(started: Arc<Started>) {
    loop {
        if let Err(error) = due(&started).await {
            tracing::error!(error = %error, "the sender could not read its deliveries");
        }
        if let Err(error) = direct::due(&started).await {
            tracing::error!(error = %error, "the sender could not read its direct deliveries");
        }
        tokio::select! {
            () = started.woken() => {}
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

async fn due(started: &Started) -> Result<usize> {
    let now = started.clock.now();
    let rows = delivery::Entity::find()
        .filter(delivery::Column::State.eq(STATE_PENDING))
        .filter(delivery::Column::NextAt.lte(now))
        .order_by_asc(delivery::Column::NextAt)
        .order_by_asc(delivery::Column::Id)
        .limit(BATCH)
        .all(&started.db)
        .await?;
    let count = rows.len();
    for row in rows {
        attempt(started, row).await?;
    }
    Ok(count)
}

async fn attempt(started: &Started, row: delivery::Model) -> Result<()> {
    let outcome = send(started, &row).await;
    let now = started.clock.now();
    let attempt = row.attempt + 1;
    match outcome {
        Ok(()) => {
            delivery::Entity::update_many()
                .filter(delivery::Column::Id.eq(row.id.clone()))
                .col_expr(delivery::Column::State, Expr::value(STATE_SENT))
                .col_expr(delivery::Column::Attempt, Expr::value(attempt))
                .col_expr(delivery::Column::SentAt, Expr::value(Some(now)))
                .col_expr(delivery::Column::Error, Expr::value(None::<String>))
                .exec(&started.db)
                .await?;
            channel_sent(&started.db, &row.channel_id, now).await?;
            tracing::info!(
                delivery = row.id,
                channel = row.channel,
                key = row.message_key,
                "delivered"
            );
        }
        Err(error) => {
            let reason = reason_of(&error);
            tracing::warn!(delivery = row.id, channel = row.channel, attempt, error = %reason, "the delivery failed");
            let next = next_attempt_at(attempt, now);
            let state = if next.is_some() {
                STATE_PENDING
            } else {
                STATE_FAILED
            };
            delivery::Entity::update_many()
                .filter(delivery::Column::Id.eq(row.id.clone()))
                .col_expr(delivery::Column::State, Expr::value(state))
                .col_expr(delivery::Column::Attempt, Expr::value(attempt))
                .col_expr(delivery::Column::NextAt, Expr::value(next.unwrap_or(now)))
                .col_expr(delivery::Column::Error, Expr::value(Some(reason.clone())))
                .exec(&started.db)
                .await?;
            channel_failed(&started.db, &row.channel_id, &reason).await?;
            if next.is_none() {
                dead_letter(
                    started.caller.as_ref(),
                    &Letter {
                        channel_id: &row.channel_id,
                        channel: &row.channel,
                        delivery: &row.id,
                        key: &row.message_key,
                    },
                    &reason,
                )
                .await;
            }
        }
    }
    Ok(())
}

pub struct Letter<'a> {
    pub channel_id: &'a str,
    pub channel: &'a str,
    pub delivery: &'a str,
    pub key: &'a str,
}

pub async fn channel_sent(db: &Db, channel_id: &str, now: DateTime<Utc>) -> Result<()> {
    channel::Entity::update_many()
        .filter(channel::Column::Id.eq(channel_id))
        .col_expr(channel::Column::LastSentAt, Expr::value(Some(now)))
        .col_expr(channel::Column::LastError, Expr::value(None::<String>))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn channel_failed(db: &Db, channel_id: &str, reason: &str) -> Result<()> {
    channel::Entity::update_many()
        .filter(channel::Column::Id.eq(channel_id))
        .col_expr(channel::Column::LastError, Expr::value(Some(reason)))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn mail_channel(
    started: &Started,
    id: &str,
) -> Result<(channel::Model, StoredEmail, Option<Secret>)> {
    let channel = channel::Entity::find_by_id(id.to_owned())
        .one(&started.db)
        .await?
        .ok_or_else(|| KeelError::not_found(ERROR_CHANNEL_GONE).param("id", id))?;
    let kind = kind_of(&channel)?;
    if kind != ChannelKind::Email {
        return Err(KeelError::validation(ERROR_KIND_UNAVAILABLE).param("kind", kind.as_str()));
    }
    let email = email_of(&channel)?;
    let password = email.open_password(started.cipher.as_ref())?;
    Ok((channel, email, password))
}

pub async fn recipient(started: &Started, principal: &str) -> Result<(String, String)> {
    let found = match principal.parse::<Id>() {
        Ok(id) => started.principals.by_id(id, &started.db).await?,
        Err(_) => None,
    };
    Ok(found.map_or_else(
        || {
            (
                super::super::cron::api::OWNER_LOGIN.to_owned(),
                "en".to_owned(),
            )
        },
        |principal| (principal.login, principal.lang),
    ))
}

pub async fn dead_letter(caller: &dyn Caller, letter: &Letter<'_>, reason: &str) {
    let raise = Raise::new(PROBLEM_KEY, letter.channel_id, Severity::Warning)
        .params(json!({
            "channel": letter.channel,
            "channelId": letter.channel_id,
            "delivery": letter.delivery,
            "key": letter.key,
            "error": reason,
        }))
        .map(|raise| raise.source(PROBLEM_SOURCE));
    let raised = match raise {
        Ok(raise) => Problems::raise(caller, raise).await,
        Err(error) => Err(error),
    };
    if let Err(error) = raised {
        tracing::error!(delivery = letter.delivery, error = %error, "the dead letter could not be raised as a problem");
    }
}

async fn send(started: &Started, row: &delivery::Model) -> Result<()> {
    let (channel, email, password) = mail_channel(started, &row.channel_id).await?;
    let message = message::Entity::find_by_id(row.message_id.clone())
        .one(&started.db)
        .await?
        .ok_or_else(|| {
            KeelError::not_found(ERROR_MESSAGE_GONE).param("id", row.message_id.as_str())
        })?;
    let (login, lang) = recipient(started, &message.principal_id).await?;
    let context = json!({
        "key": message.key,
        "level": message.level,
        "source": message.source,
        "principal": login,
        "lang": lang,
        "channel": channel.name,
        "params": message.params_json,
    });
    let rendered = started.templates.render(&message.key, &lang, &context)?;
    email::send(
        &email,
        password,
        &Mail {
            to: &email.to,
            subject: &rendered.subject,
            body: &rendered.body,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_waits_a_minute_then_longer_and_ends_after_the_sixth_attempt() {
        let now: DateTime<Utc> = "2026-09-15T10:00:00Z".parse().expect("an instant");
        let waits: Vec<Option<i64>> = (1..=MAX_ATTEMPTS)
            .map(|attempt| next_attempt_at(attempt, now).map(|at| (at - now).num_seconds()))
            .collect();
        assert_eq!(
            waits,
            [
                Some(60),
                Some(300),
                Some(1800),
                Some(7200),
                Some(43200),
                None
            ]
        );
        assert_eq!(MAX_ATTEMPTS, 6);
        assert_eq!(
            next_attempt_at(0, now),
            Some(now + keel_sdk::chrono::Duration::seconds(60))
        );
        let error = KeelError::unavailable("notify.error.email_send").param("reason", "refused");
        assert_eq!(reason_of(&error), "notify.error.email_send: refused");
        let bare = KeelError::not_found(ERROR_CHANNEL_GONE).param("id", "01J");
        assert_eq!(
            reason_of(&bare),
            "not_found: notify.error.channel_gone {\"id\":\"01J\"}"
        );
    }
}
