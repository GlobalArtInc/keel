use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderValue;
use axum::http::header::CACHE_CONTROL;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use futures::{StreamExt, stream};
use keel_sdk::Lines;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Client, Credential};
use keel_types::{LogLine, Principal};
use sea_orm::DatabaseConnection;
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::auth;
use super::events::{KEEP_ALIVE_COMMENT, Opened, Streams, draining, session_key};
use super::live::Resolvers;

pub const LINE_EVENT: &str = "line";
pub const END_EVENT: &str = "end";

const NO_STORE: &str = "no-store";
const QUEUE: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Done,
    Expired,
    Superseded,
    Draining,
}

impl End {
    fn reason(self) -> &'static str {
        match self {
            End::Done => "done",
            End::Expired => "expired",
            End::Superseded => "superseded",
            End::Draining => "draining",
        }
    }
}

pub(super) struct Tail {
    pub(super) function: String,
    pub(super) request_id: String,
    pub(super) principal: Principal,
    pub(super) credential: Credential,
    pub(super) on_behalf: Option<String>,
    pub(super) client: Client,
    pub(super) resolvers: Resolvers,
    pub(super) keep_alive: Duration,
    pub(super) db: DatabaseConnection,
}

pub(super) fn serve(streams: &Arc<Streams>, tail: Tail, lines: Lines) -> Response {
    let (opened, evicted) = streams.enter(session_key(&tail.credential));
    let drained = streams.drained();
    let (tx, rx) = mpsc::channel(QUEUE);
    tracing::debug!(
        request_id = tail.request_id,
        function = tail.function,
        principal = %tail.principal.id,
        "function stream opened"
    );
    tokio::spawn(run(tail, lines, tx, drained, evicted, opened));
    let body = stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|event| (Ok::<SseEvent, Infallible>(event), rx))
    });
    let mut response = Sse::new(body).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(NO_STORE));
    response
}

async fn run(
    tail: Tail,
    mut lines: Lines,
    tx: mpsc::Sender<SseEvent>,
    mut drained: watch::Receiver<bool>,
    mut evicted: oneshot::Receiver<()>,
    opened: Opened,
) {
    let end = pump(&tail, &mut lines, &tx, &mut drained, &mut evicted).await;
    if let Some(end) = end {
        let _ = tx.send(end_event(end)).await;
    }
    tracing::debug!(
        request_id = tail.request_id,
        function = tail.function,
        principal = %tail.principal.id,
        reason = end.map_or("closed", End::reason),
        "function stream closed"
    );
    drop(opened);
}

async fn pump(
    tail: &Tail,
    lines: &mut Lines,
    tx: &mpsc::Sender<SseEvent>,
    drained: &mut watch::Receiver<bool>,
    evicted: &mut oneshot::Receiver<()>,
) -> Option<End> {
    let mut keep_alive =
        tokio::time::interval_at(Instant::now() + tail.keep_alive, tail.keep_alive);
    keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = tx.closed() => return None,
            () = draining(drained) => return Some(End::Draining),
            superseded = &mut *evicted => return superseded.is_ok().then_some(End::Superseded),
            line = lines.next() => match line {
                Some(line) => {
                    if let Some(event) = line_event(&line)
                        && tx.send(event).await.is_err()
                    {
                        return None;
                    }
                }
                None => return Some(End::Done),
            },
            _ = keep_alive.tick() => {
                if !still_admitted(tail).await {
                    return Some(End::Expired);
                }
                if tx.send(SseEvent::default().comment(KEEP_ALIVE_COMMENT)).await.is_err() {
                    return None;
                }
            }
        }
    }
}

async fn still_admitted(tail: &Tail) -> bool {
    let db = Db::from_connection(tail.db.clone());
    match auth::resolve(
        &tail.resolvers,
        &tail.credential,
        tail.on_behalf.as_deref(),
        &tail.client,
        &db,
    )
    .await
    {
        Ok(authenticated) => {
            authenticated.principal.id == tail.principal.id
                && authenticated.principal.level == tail.principal.level
        }
        Err(error) if auth::refuses(&error) => false,
        Err(error) => {
            tracing::warn!(
                request_id = tail.request_id,
                function = tail.function,
                error = %error,
                "the function stream's credential could not be checked again"
            );
            true
        }
    }
}

fn line_event(line: &LogLine) -> Option<SseEvent> {
    serde_json::to_string(line)
        .ok()
        .map(|data| SseEvent::default().event(LINE_EVENT).data(data))
}

fn end_event(end: End) -> SseEvent {
    SseEvent::default()
        .event(END_EVENT)
        .data(json!({ "reason": end.reason() }).to_string())
}

#[cfg(test)]
mod tests {
    use keel_types::LogLevel;

    use super::*;

    #[test]
    fn every_end_names_its_reason() {
        let reasons: Vec<&str> = [End::Done, End::Expired, End::Superseded, End::Draining]
            .into_iter()
            .map(End::reason)
            .collect();
        assert_eq!(reasons, ["done", "expired", "superseded", "draining"]);
    }

    #[test]
    fn a_line_becomes_a_line_event_with_its_json() {
        assert!(line_event(&LogLine::new(LogLevel::Info, "a\nb")).is_some());
    }
}
