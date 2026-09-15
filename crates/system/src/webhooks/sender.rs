use std::error::Error as StdError;
use std::sync::Arc;
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use keel_sdk::serde_json;
use keel_sdk::types::Event;
use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;

use super::super::notify::api::{Problems, Raise, Severity};
use super::consumer::{STATE_DEAD, STATE_DELIVERED, STATE_FAILED, STATE_PENDING};
use super::entity::{delivery, subscription};
use super::signature::{self, EVENT_ID_HEADER, HEADER};
use super::started::Started;
use super::targets;

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
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const BODY_CAP: usize = 1 << 20;
pub const ERROR_CAP: usize = 512;
pub const CONTENT_TYPE_JSON: &str = "application/json";
pub const PROBLEM_KEY: &str = "webhook.dead_letter";
pub const PROBLEM_SOURCE: &str = "webhook.delivery";
pub const ERROR_SUBSCRIPTION_GONE: &str = "webhook.error.subscription_gone";
pub const ERROR_EVENT_GONE: &str = "webhook.error.event_gone";
pub const ERROR_SECRET: &str = "webhook.error.secret";
pub const ERROR_CLIENT: &str = "webhook.error.client";
pub const ERROR_SEND: &str = "webhook.error.send";
pub const ERROR_STATUS: &str = "webhook.error.status";

pub fn next_attempt_at(attempt: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    let index = usize::try_from(attempt.saturating_sub(1)).unwrap_or_default();
    let wait = LADDER.get(index).copied()?;
    let wait = keel_sdk::chrono::Duration::from_std(wait).ok()?;
    Some(now + wait)
}

pub fn truncate(text: &str, cap: usize) -> String {
    if text.len() <= cap {
        return text.to_owned();
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

pub fn describe(error: &(dyn StdError + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        let line = inner.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = inner.source();
    }
    text
}

pub fn reason_of(error: &KeelError) -> String {
    let reason = error.params().get("reason").and_then(Value::as_str);
    match reason {
        Some(reason) => format!("{}: {reason}", error.key()),
        None => error.key().to_owned(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub status: Option<u16>,
    pub error: String,
}

impl From<KeelError> for Failed {
    fn from(error: KeelError) -> Self {
        Failed {
            status: None,
            error: reason_of(&error),
        }
    }
}

pub async fn run(started: Arc<Started>) {
    loop {
        if let Err(error) = due(&started).await {
            tracing::error!(error = %error, "the webhook sender could not read its deliveries");
        }
        tokio::select! {
            () = started.woken() => {}
            () = tokio::time::sleep(POLL) => {}
        }
    }
}

async fn due(started: &Started) -> Result<usize> {
    let now = started.seams.clock.now();
    let rows = delivery::Entity::find()
        .filter(delivery::Column::State.is_in([STATE_PENDING, STATE_FAILED]))
        .filter(delivery::Column::NextAt.lte(now))
        .order_by_asc(delivery::Column::NextAt)
        .order_by_asc(delivery::Column::Id)
        .limit(BATCH)
        .all(started.db())
        .await?;
    let count = rows.len();
    for row in rows {
        attempt(started, row).await?;
    }
    Ok(count)
}

async fn attempt(started: &Started, row: delivery::Model) -> Result<()> {
    let subscription = subscription::Entity::find_by_id(row.subscription_id.clone())
        .one(started.db())
        .await?;
    let Some(subscription) = subscription else {
        settle(
            started,
            &row,
            None,
            Err(Failed {
                status: None,
                error: ERROR_SUBSCRIPTION_GONE.to_owned(),
            }),
        )
        .await?;
        return Ok(());
    };
    if !subscription.enabled {
        return Ok(());
    }
    let outcome = post(started, &row, &subscription).await;
    settle(started, &row, Some(&subscription), outcome).await
}

async fn settle(
    started: &Started,
    row: &delivery::Model,
    subscription: Option<&subscription::Model>,
    outcome: Result<u16, Failed>,
) -> Result<()> {
    let now = started.seams.clock.now();
    let attempt = row.attempt + 1;
    match outcome {
        Ok(status) => {
            delivery::Entity::update_many()
                .filter(delivery::Column::Id.eq(row.id.clone()))
                .col_expr(delivery::Column::State, Expr::value(STATE_DELIVERED))
                .col_expr(delivery::Column::Attempt, Expr::value(attempt))
                .col_expr(
                    delivery::Column::LastStatus,
                    Expr::value(Some(i32::from(status))),
                )
                .col_expr(delivery::Column::LastError, Expr::value(None::<String>))
                .col_expr(delivery::Column::DeliveredAt, Expr::value(Some(now)))
                .exec(started.db())
                .await?;
            tracing::info!(
                delivery = row.id,
                subscription = row.subscription_id,
                event = row.event_id,
                status,
                "delivered"
            );
            if row.attempt >= MAX_ATTEMPTS
                && let Some(subscription) = subscription
            {
                revive(started, subscription).await;
            }
        }
        Err(failed) => {
            let error = truncate(&failed.error, ERROR_CAP);
            tracing::warn!(
                delivery = row.id,
                subscription = row.subscription_id,
                attempt,
                status = failed.status,
                error,
                "the delivery failed"
            );
            let next = next_attempt_at(attempt, now);
            let state = if next.is_some() {
                STATE_FAILED
            } else {
                STATE_DEAD
            };
            delivery::Entity::update_many()
                .filter(delivery::Column::Id.eq(row.id.clone()))
                .col_expr(delivery::Column::State, Expr::value(state))
                .col_expr(delivery::Column::Attempt, Expr::value(attempt))
                .col_expr(delivery::Column::NextAt, Expr::value(next.unwrap_or(now)))
                .col_expr(
                    delivery::Column::LastStatus,
                    Expr::value(failed.status.map(i32::from)),
                )
                .col_expr(
                    delivery::Column::LastError,
                    Expr::value(Some(error.clone())),
                )
                .exec(started.db())
                .await?;
            if next.is_none()
                && let Some(subscription) = subscription
            {
                dead_letter(started, row, subscription, &error).await;
            }
        }
    }
    Ok(())
}

async fn dead_letter(
    started: &Started,
    row: &delivery::Model,
    subscription: &subscription::Model,
    error: &str,
) {
    let raise = Raise::new(PROBLEM_KEY, &subscription.id, Severity::Warning)
        .params(json!({
            "subscription": subscription.id,
            "name": subscription.name,
            "url": subscription.url,
            "delivery": row.id,
            "event": row.event_id,
            "type": row.kind,
            "error": error,
        }))
        .map(|raise| raise.source(PROBLEM_SOURCE))
        .map(|raise| match subscription.account_id.parse::<Id>() {
            Ok(account) => raise.account(account),
            Err(_) => raise,
        });
    let raised = match raise {
        Ok(raise) => Problems::raise(started.seams.caller.as_ref(), raise).await,
        Err(error) => Err(error),
    };
    if let Err(error) = raised {
        tracing::error!(delivery = row.id, error = %error, "the dead letter could not be raised as a problem");
    }
}

async fn revive(started: &Started, subscription: &subscription::Model) {
    if let Err(error) =
        Problems::clear(started.seams.caller.as_ref(), PROBLEM_KEY, &subscription.id).await
    {
        tracing::error!(subscription = subscription.id, error = %error, "the dead letter could not be cleared");
    }
}

async fn event_of(started: &Started, row: &delivery::Model) -> Result<Event, Failed> {
    let before = u64::try_from(row.seq).unwrap_or_default().saturating_sub(1);
    let events = started.seams.subscriber.after(before, 1).await?;
    events
        .into_iter()
        .find(|event| event.id.to_string() == row.event_id)
        .ok_or_else(|| Failed {
            status: None,
            error: ERROR_EVENT_GONE.to_owned(),
        })
}

async fn post(
    started: &Started,
    row: &delivery::Model,
    subscription: &subscription::Model,
) -> Result<u16, Failed> {
    let event = event_of(started, row).await?;
    let body = serde_json::to_vec(&event).map_err(|error| Failed {
        status: None,
        error: describe(&error),
    })?;
    let allow_private = started.private_targets().await?;
    let target = targets::check(
        &subscription.url,
        allow_private,
        started.seams.resolver.as_ref(),
    )
    .await?;
    let secret = started
        .seams
        .cipher
        .open(&subscription.secret)
        .map_err(|error| Failed {
            status: None,
            error: format!("{ERROR_SECRET}: {}", reason_of(&error)),
        })?;
    let t = started.seams.clock.now().timestamp();
    let signature = signature::sign(&secret, t, &body).map_err(|error| Failed {
        status: None,
        error: format!("{ERROR_SECRET}: {error}"),
    })?;
    let mut builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT);
    if target.pinned {
        builder = builder.resolve_to_addrs(&target.host, &target.addrs);
    }
    let client = builder.build().map_err(|error| Failed {
        status: None,
        error: format!("{ERROR_CLIENT}: {}", describe(&error)),
    })?;
    let response = client
        .post(target.url.clone())
        .header(CONTENT_TYPE, CONTENT_TYPE_JSON)
        .header(EVENT_ID_HEADER, event.id.to_string())
        .header(HEADER, signature)
        .body(body)
        .send()
        .await
        .map_err(|error| Failed {
            status: None,
            error: format!("{ERROR_SEND}: {}", describe(&error)),
        })?;
    let status = response.status().as_u16();
    let success = response.status().is_success();
    let snippet = drain(response).await;
    if success {
        Ok(status)
    } else {
        Err(Failed {
            status: Some(status),
            error: format!("{ERROR_STATUS}: {status} {snippet}"),
        })
    }
}

async fn drain(mut response: reqwest::Response) -> String {
    let mut read = Vec::new();
    while read.len() < BODY_CAP {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                read.extend_from_slice(&chunk[..chunk.len().min(BODY_CAP - read.len())])
            }
            Ok(None) | Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&read);
    truncate(text.trim(), ERROR_CAP / 2)
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
    }

    #[test]
    fn an_error_text_is_cut_at_a_character_boundary_and_a_failure_names_its_key_and_reason() {
        assert_eq!(truncate("short", 512), "short");
        let long = "é".repeat(300);
        let cut = truncate(&long, 512);
        assert_eq!(cut.len(), 512);
        assert_eq!(cut.chars().count(), 256);
        assert_eq!(truncate("abc", 2), "ab");
        let error = KeelError::validation(targets::ERROR_URL_PRIVATE)
            .field("url")
            .param("reason", "private_address");
        assert_eq!(
            Failed::from(error),
            Failed {
                status: None,
                error: "webhook.error.url_private: private_address".to_owned()
            }
        );
        let bare = KeelError::not_found(ERROR_EVENT_GONE);
        assert_eq!(reason_of(&bare), ERROR_EVENT_GONE);
        let io = std::io::Error::other("connection refused");
        assert_eq!(describe(&io), "connection refused");
    }
}
