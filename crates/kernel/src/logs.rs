use std::collections::VecDeque;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, Utc};
use futures::Stream;
use futures::stream;
use keel_types::LogLine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

pub const CAPACITY: usize = 10_000;
const FOLLOW_BATCH: usize = 256;
const MESSAGE_FIELD: &str = "message";

pub use keel_types::LogLevel;

fn level_of(level: tracing::Level) -> LogLevel {
    match level {
        tracing::Level::TRACE => LogLevel::Trace,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::ERROR => LogLevel::Error,
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub seq: u64,
    pub at: DateTime<Utc>,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

impl Entry {
    pub(crate) fn line(&self) -> LogLine {
        let mut text = format!("{} {}", self.target, self.message);
        for (name, value) in &self.fields {
            match value {
                Value::String(value) => text.push_str(&format!(" {name}={value}")),
                other => text.push_str(&format!(" {name}={other}")),
            }
        }
        LogLine::new(self.level, text).at(self.at)
    }
}

struct Inner {
    entries: VecDeque<Entry>,
    next: u64,
    dropped: u64,
}

pub struct Ring {
    inner: Mutex<Inner>,
    capacity: usize,
    pushed: Notify,
}

impl Ring {
    const fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: VecDeque::new(),
                next: 1,
                dropped: 0,
            }),
            capacity,
            pushed: Notify::const_new(),
        }
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn push(&self, mut entry: Entry) -> u64 {
        let mut inner = self.inner();
        let seq = inner.next;
        inner.next += 1;
        entry.seq = seq;
        if inner.entries.len() >= self.capacity {
            inner.entries.pop_front();
            inner.dropped += 1;
        }
        inner.entries.push_back(entry);
        drop(inner);
        self.pushed.notify_waiters();
        seq
    }

    fn backlog(&self, limit: usize) -> (VecDeque<Entry>, u64) {
        let inner = self.inner();
        let skip = inner.entries.len().saturating_sub(limit);
        let backlog = inner.entries.iter().skip(skip).cloned().collect();
        (backlog, inner.next - 1)
    }

    fn after(&self, cursor: u64, limit: usize) -> Vec<Entry> {
        let inner = self.inner();
        let start = inner.entries.partition_point(|entry| entry.seq <= cursor);
        inner
            .entries
            .iter()
            .skip(start)
            .take(limit)
            .cloned()
            .collect()
    }

    pub(crate) fn follow(&'static self, backlog: usize) -> impl Stream<Item = Entry> + Send {
        let (pending, cursor) = self.backlog(backlog);
        stream::unfold(
            (pending, cursor),
            move |(mut pending, mut cursor)| async move {
                loop {
                    if let Some(entry) = pending.pop_front() {
                        return Some((entry, (pending, cursor)));
                    }
                    let pushed = self.pushed.notified();
                    tokio::pin!(pushed);
                    pushed.as_mut().enable();
                    let fresh = self.after(cursor, FOLLOW_BATCH);
                    match fresh.last() {
                        Some(last) => {
                            cursor = last.seq;
                            pending.extend(fresh);
                        }
                        None => pushed.await,
                    }
                }
            },
        )
    }

    pub fn tail(&self, limit: usize, at_least: Option<LogLevel>) -> Vec<Entry> {
        let inner = self.inner();
        let mut newest: Vec<Entry> = inner
            .entries
            .iter()
            .rev()
            .filter(|entry| at_least.is_none_or(|floor| entry.level >= floor))
            .take(limit)
            .cloned()
            .collect();
        newest.reverse();
        newest
    }

    pub fn len(&self) -> usize {
        self.inner().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn dropped(&self) -> u64 {
        self.inner().dropped
    }
}

impl fmt::Debug for Ring {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ring")
            .field("capacity", &self.capacity)
            .field("len", &self.len())
            .field("dropped", &self.dropped())
            .finish()
    }
}

static RING: Ring = Ring::new(CAPACITY);

pub fn ring() -> &'static Ring {
    &RING
}

pub(crate) struct RingLayer {
    ring: &'static Ring,
}

impl RingLayer {
    fn new(ring: &'static Ring) -> Self {
        Self { ring }
    }
}

pub(crate) fn layer() -> RingLayer {
    RingLayer::new(ring())
}

#[derive(Default)]
struct Fields {
    message: String,
    fields: Map<String, Value>,
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == MESSAGE_FIELD {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .insert(field.name().to_owned(), Value::String(format!("{value:?}")));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == MESSAGE_FIELD {
            self.message = value.to_owned();
        } else {
            self.fields
                .insert(field.name().to_owned(), Value::String(value.to_owned()));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_owned(), Value::from(value));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .insert(field.name().to_owned(), Value::String(value.to_string()));
    }
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut visitor = Fields::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        self.ring.push(Entry {
            seq: 0,
            at: Utc::now(),
            level: level_of(*metadata.level()),
            target: metadata.target().to_owned(),
            message: visitor.message,
            fields: visitor.fields,
        });
    }
}

#[cfg(test)]
mod tests {
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    fn entry(level: LogLevel, message: &str) -> Entry {
        Entry {
            seq: 0,
            at: "2026-09-21T10:15:30.250Z".parse().unwrap(),
            level,
            target: "keel_kernel::probe".into(),
            message: message.into(),
            fields: Map::new(),
        }
    }

    #[test]
    fn the_ring_keeps_the_newest_entries_and_counts_what_it_dropped() {
        let ring = Ring::new(3);
        assert!(ring.is_empty());
        for index in 1..=5 {
            let seq = ring.push(entry(LogLevel::Info, &format!("m{index}")));
            assert_eq!(seq, index);
        }
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.capacity(), 3);
        let tail = ring.tail(10, None);
        let messages: Vec<&str> = tail.iter().map(|entry| entry.message.as_str()).collect();
        assert_eq!(messages, ["m3", "m4", "m5"]);
        assert_eq!(
            tail.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            [3, 4, 5]
        );
        let two = ring.tail(2, None);
        assert_eq!(
            two.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
            [4, 5]
        );
        assert_eq!(
            format!("{ring:?}"),
            "Ring { capacity: 3, len: 3, dropped: 2 }"
        );
    }

    #[test]
    fn the_tail_filters_by_level_and_keeps_the_order() {
        let ring = Ring::new(10);
        ring.push(entry(LogLevel::Debug, "d"));
        ring.push(entry(LogLevel::Info, "i"));
        ring.push(entry(LogLevel::Warn, "w"));
        ring.push(entry(LogLevel::Error, "e"));
        let warnings = ring.tail(10, Some(LogLevel::Warn));
        assert_eq!(
            warnings
                .iter()
                .map(|entry| entry.message.as_str())
                .collect::<Vec<_>>(),
            ["w", "e"]
        );
        let one = ring.tail(1, Some(LogLevel::Info));
        assert_eq!(one[0].message, "e");
        assert!(ring.tail(0, None).is_empty());
    }

    #[tokio::test]
    async fn the_follow_stream_yields_the_backlog_then_what_arrives() {
        use futures::StreamExt;

        let ring: &'static Ring = Box::leak(Box::new(Ring::new(8)));
        for message in ["one", "two", "three"] {
            ring.push(entry(LogLevel::Info, message));
        }
        let mut follow = Box::pin(ring.follow(2));
        let backlog: Vec<String> = [follow.next().await, follow.next().await]
            .into_iter()
            .flatten()
            .map(|entry| entry.message)
            .collect();
        assert_eq!(backlog, ["two", "three"]);
        let pending = tokio::spawn(async move { follow.next().await });
        tokio::task::yield_now().await;
        ring.push(entry(LogLevel::Error, "four"));
        let arrived = pending.await.unwrap().expect("a new entry");
        assert_eq!(arrived.message, "four");
        assert_eq!(arrived.seq, 4);
        let line = arrived.line();
        assert_eq!(line.level, LogLevel::Error);
        assert_eq!(line.text, "keel_kernel::probe four");
        assert_eq!(line.at, Some(arrived.at));
    }

    #[test]
    fn a_level_comes_from_tracing_and_serialises_by_its_name() {
        assert!(LogLevel::Error > LogLevel::Trace);
        assert_eq!(level_of(tracing::Level::DEBUG), LogLevel::Debug);
        assert_eq!(level_of(tracing::Level::TRACE), LogLevel::Trace);
        assert_eq!(
            serde_json::to_value(LogLevel::Info).unwrap(),
            Value::String("info".into())
        );
    }

    #[test]
    fn an_entry_serialises_with_its_fields_and_comes_back() {
        let mut fields = Map::new();
        fields.insert("count".into(), Value::from(3));
        fields.insert("who".into(), Value::String("alice".into()));
        let entry = Entry {
            fields,
            ..entry(LogLevel::Warn, "slow")
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["level"], "warn");
        assert_eq!(json["fields"]["count"], 3);
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn the_layer_records_events_with_their_message_fields_and_target() {
        static LOCAL: Ring = Ring::new(16);
        let subscriber = tracing_subscriber::registry().with(RingLayer::new(&LOCAL));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(count = 2u64, flag = true, name = "x", "first {}", "message");
            tracing::warn!(error = %std::io::Error::other("boom"), "second");
            tracing::debug!(ratio = 0.5f64, signed = -1i64, "third");
        });
        let tail = LOCAL.tail(10, None);
        assert_eq!(tail.len(), 3);
        assert_eq!(tail[0].message, "first message");
        assert_eq!(tail[0].level, LogLevel::Info);
        assert_eq!(tail[0].target, "keel_kernel::logs::tests");
        assert_eq!(tail[0].fields["count"], 2);
        assert_eq!(tail[0].fields["flag"], true);
        assert_eq!(tail[0].fields["name"], "x");
        assert_eq!(tail[1].message, "second");
        assert_eq!(tail[1].fields["error"], "boom");
        assert_eq!(tail[2].fields["ratio"], 0.5);
        assert_eq!(tail[2].fields["signed"], -1);
        assert!(tail[0].seq < tail[1].seq && tail[1].seq < tail[2].seq);
        assert_eq!(ring().capacity(), CAPACITY);
    }
}
