use std::time::Duration;

use keel_types::{ErrorCode, Event, Id};
use reqwest::header::{CACHE_CONTROL, CONTENT_TYPE, HeaderName};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::Suite;
use crate::client::Credential;
use crate::report::{Aborted, Failure, ensure, fail};

const PATH: &str = "/api/v1/events";
const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
const EVENT_STREAM: &str = "text/event-stream";
const NO_STORE: &str = "no-store";
const DATA_INVALIDATE: &str = "data.invalidate";
const RETRY_MS: u64 = 3000;
const STREAMS_PER_SESSION: usize = 4;
const KEEP_ALIVE_SECONDS: u64 = 15;
const FRAME_WAIT: Duration = Duration::from_secs(10);
const CLOSE_WAIT: Duration = Duration::from_secs(KEEP_ALIVE_SECONDS + 10);
const ADMIN_NOTE: &str = "conformance-events-admin";
const USER_NOTE: &str = "conformance-events-user";
const USER_NOTE_EDITED: &str = "conformance-events-user-edited";
const SESSION_NOTE: &str = "conformance-events-session";

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let answer = suite.client.get(PATH, Credential::Guest, &[]).await;
    let _ = suite.report.check(
        "GET /api/v1/events as a guest answers 401 unauthenticated",
        answer.and_then(|answer| {
            answer
                .error(
                    401,
                    ErrorCode::Unauthenticated,
                    "keel.error.unauthenticated",
                )
                .map(|_| ())
        }),
    );
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the event checks", outcome)?;
    let outcome = Stream::open(suite, None).await;
    let mut admin = suite.report.check(
        "the admin's session opens the stream as text/event-stream, no-store, retry: 3000",
        outcome,
    )?;
    let outcome = edit(suite, None, ADMIN_NOTE).await;
    let admin_note = suite
        .report
        .check("note.edit creates the admin's note", outcome)?;
    let outcome = admin.invalidation(admin_note).await;
    let (admin_seq, _) = suite.report.check(
        "the admin's stream receives the data.invalidate of its note.edit with the seq as its id",
        outcome,
    )?;
    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the event checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the event checks", outcome)?;
    let outcome = edit(suite, None, USER_NOTE).await;
    let user_note = suite
        .report
        .check("note.edit as the user creates the user's note", outcome)?;
    let outcome = Stream::open(suite, Some(admin_seq.saturating_sub(1))).await;
    let mut own = suite.report.check(
        "the user opens the stream with Last-Event-ID just before the admin's note",
        outcome,
    )?;
    let outcome = own.invalidation(user_note).await.and_then(|(seq, before)| {
        ensure(seq > admin_seq, || {
            format!("the user's note has seq {seq}, not after {admin_seq}")
        })?;
        ensure(!before.iter().any(|event| about(event, admin_note)), || {
            "the admin's note was replayed to the user".to_owned()
        })
    });
    let _ = suite.report.check(
        "Last-Event-ID replays the user's own note within the window and leaves the admin's out",
        outcome,
    );
    let outcome = admin.invalidation(user_note).await;
    let _ = suite.report.check(
        "the admin's stream receives the data.invalidate of the user's note too",
        outcome.map(|_| ()),
    );
    let outcome = edit(suite, Some(user_note), USER_NOTE_EDITED).await;
    let _ = suite.report.check(
        "note.edit as the user edits its note while the stream is live",
        outcome.map(|_| ()),
    );
    let outcome = own.invalidation(user_note).await;
    let _ = suite.report.check(
        "the user's stream receives the data.invalidate of its own edit live",
        outcome.map(|_| ()),
    );
    drop((admin, own));
    let outcome = suite.su_exit().await;
    suite.report.check(
        "auth.su.exit back to the admin after the event checks",
        outcome,
    )?;
    let outcome = suite.logout().await;
    suite.report.check(
        "auth.logout ends the first session of the event checks",
        outcome,
    )?;

    let outcome = suite.login().await;
    suite.report.check(
        "auth.login opens a fresh session for the cap and the revocation",
        outcome,
    )?;
    let outcome = open_many(suite, STREAMS_PER_SESSION + 1).await;
    let mut streams = suite
        .report
        .check("one session opens five streams in a row", outcome)?;
    let mut first = streams.remove(0);
    let outcome = first.superseded().await;
    let _ = suite.report.check(
        "the fifth stream of a session closes the first with retry: 0",
        outcome,
    );
    let outcome = edit(suite, None, SESSION_NOTE).await;
    let session_note = suite.report.check(
        "note.edit creates a note for the session's newer streams",
        outcome,
    )?;
    let outcome = all_receive(&mut streams, session_note).await;
    let _ = suite.report.check(
        "the four newer streams of the session stay open and receive its data.invalidate",
        outcome,
    );
    let answer = suite
        .client
        .call(
            "note.delete",
            json!({ "ids": [admin_note, user_note, session_note] }),
            Credential::Session,
            &[],
        )
        .await;
    let _ = suite.report.check(
        "note.delete removes the notes of the event checks",
        answer.and_then(|answer| answer.data::<Value>().map(|_| ())),
    );
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout revokes the session the streams hold", outcome)?;
    let outcome = all_close(&mut streams).await;
    let _ = suite.report.check(
        "every stream of the revoked session closes at its next keep-alive",
        outcome,
    );
    Ok(())
}

async fn edit(suite: &mut Suite, id: Option<Id>, title: &str) -> Result<Id, Failure> {
    let mut params = json!({ "title": title });
    if let Some(id) = id {
        params["id"] = json!(id);
    }
    let answer = suite
        .client
        .call("note.edit", params, Credential::Session, &[])
        .await?;
    let (row, _) = answer.data::<Value>()?;
    super::id_of(&row, "id").map_err(|failure| answer.failure(failure.0))
}

async fn open_many(suite: &mut Suite, count: usize) -> Result<Vec<Stream>, Failure> {
    let mut streams = Vec::with_capacity(count);
    for _ in 0..count {
        streams.push(Stream::open(suite, None).await?);
    }
    Ok(streams)
}

async fn all_receive(streams: &mut [Stream], subject: Id) -> Result<(), Failure> {
    for (index, stream) in streams.iter_mut().enumerate() {
        stream
            .invalidation(subject)
            .await
            .map_err(|failure| fail(format!("stream {}: {}", index + 2, failure.0)))?;
    }
    Ok(())
}

async fn all_close(streams: &mut [Stream]) -> Result<(), Failure> {
    let deadline = Instant::now() + CLOSE_WAIT;
    for (index, stream) in streams.iter_mut().enumerate() {
        let within = deadline.saturating_duration_since(Instant::now());
        ensure(stream.closes(within).await?, || {
            format!(
                "stream {} is still open {CLOSE_WAIT:?} after the logout",
                index + 2
            )
        })?;
    }
    Ok(())
}

fn about(event: &Event, subject: Id) -> bool {
    event
        .subject
        .as_ref()
        .is_some_and(|about| about.id == subject)
}

#[derive(Debug, Default)]
struct Frame {
    event: Option<String>,
    id: Option<String>,
    data: Vec<String>,
    retry: Option<u64>,
}

enum Read {
    Frame(Frame),
    Closed,
    Silent,
}

struct Stream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    closed: bool,
}

impl Stream {
    async fn open(suite: &mut Suite, last_event_id: Option<u64>) -> Result<Self, Failure> {
        let id = last_event_id.map(|id| id.to_string());
        let extra: Vec<(HeaderName, &str)> =
            id.iter().map(|id| (LAST_EVENT_ID, id.as_str())).collect();
        let response = suite
            .client
            .stream(PATH, Credential::Session, &extra)
            .await?;
        let status = response.status().as_u16();
        ensure(status == 200, || format!("GET {PATH} answered {status}"))?;
        let header = |name: HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        let content_type = header(CONTENT_TYPE);
        ensure(content_type == EVENT_STREAM, || {
            format!("Content-Type is {content_type}")
        })?;
        let cache_control = header(CACHE_CONTROL);
        ensure(cache_control == NO_STORE, || {
            format!("Cache-Control is {cache_control}")
        })?;
        let mut stream = Self {
            response,
            buffer: Vec::new(),
            closed: false,
        };
        match stream.read(FRAME_WAIT).await? {
            Read::Frame(frame) if frame.retry == Some(RETRY_MS) => Ok(stream),
            Read::Frame(frame) => Err(fail(format!(
                "the first frame is {frame:?}, not retry: {RETRY_MS}"
            ))),
            Read::Closed => Err(fail("the stream closed before its first frame")),
            Read::Silent => Err(fail(format!("no first frame within {FRAME_WAIT:?}"))),
        }
    }

    async fn read(&mut self, within: Duration) -> Result<Read, Failure> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(frame) = self.parse() {
                return Ok(Read::Frame(frame));
            }
            if self.closed {
                return Ok(Read::Closed);
            }
            match tokio::time::timeout_at(deadline, self.response.chunk()).await {
                Err(_) => return Ok(Read::Silent),
                Ok(chunk) => match chunk? {
                    Some(bytes) => self.buffer.extend_from_slice(&bytes),
                    None => self.closed = true,
                },
            }
        }
    }

    fn parse(&mut self) -> Option<Frame> {
        let end = self.buffer.windows(2).position(|pair| pair == b"\n\n")?;
        let block: Vec<u8> = self.buffer.drain(..end + 2).collect();
        let text = String::from_utf8_lossy(&block[..end]);
        let mut frame = Frame::default();
        for line in text.lines() {
            if line.starts_with(':') {
                continue;
            }
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => frame.event = Some(value.to_owned()),
                "id" => frame.id = Some(value.to_owned()),
                "data" => frame.data.push(value.to_owned()),
                "retry" => frame.retry = value.parse().ok(),
                _ => {}
            }
        }
        Some(frame)
    }

    async fn invalidation(&mut self, subject: Id) -> Result<(u64, Vec<Event>), Failure> {
        let deadline = Instant::now() + FRAME_WAIT;
        let mut before = Vec::new();
        loop {
            let within = deadline.saturating_duration_since(Instant::now());
            match self.read(within).await? {
                Read::Frame(frame) if frame.event.as_deref() == Some(DATA_INVALIDATE) => {
                    let (seq, event) = invalidated(&frame)?;
                    if about(&event, subject) {
                        return Ok((seq, before));
                    }
                    before.push(event);
                }
                Read::Frame(_) => {}
                Read::Closed => {
                    return Err(fail(format!(
                        "the stream closed before a {DATA_INVALIDATE} about {subject}"
                    )));
                }
                Read::Silent => {
                    return Err(fail(format!(
                        "no {DATA_INVALIDATE} about {subject} within {FRAME_WAIT:?}"
                    )));
                }
            }
        }
    }

    async fn superseded(&mut self) -> Result<(), Failure> {
        loop {
            match self.read(FRAME_WAIT).await? {
                Read::Frame(frame) if frame.retry.is_some() => {
                    ensure(frame.retry == Some(0), || {
                        format!("the final frame is {frame:?}")
                    })?;
                    return ensure(self.closes(FRAME_WAIT).await?, || {
                        "the stream stayed open after retry: 0".to_owned()
                    });
                }
                Read::Frame(_) => {}
                Read::Closed => return Err(fail("the stream closed without a retry: 0 frame")),
                Read::Silent => {
                    return Err(fail(format!("no retry: 0 frame within {FRAME_WAIT:?}")));
                }
            }
        }
    }

    async fn closes(&mut self, within: Duration) -> Result<bool, Failure> {
        let deadline = Instant::now() + within;
        loop {
            let within = deadline.saturating_duration_since(Instant::now());
            match self.read(within).await? {
                Read::Frame(_) => {}
                Read::Closed => return Ok(true),
                Read::Silent => return Ok(false),
            }
        }
    }
}

fn invalidated(frame: &Frame) -> Result<(u64, Event), Failure> {
    let event: Event = serde_json::from_str(&frame.data.join("\n"))?;
    let id = frame
        .id
        .as_deref()
        .ok_or_else(|| fail(format!("a {DATA_INVALIDATE} without an id: {frame:?}")))?;
    let seq: u64 = id
        .parse()
        .map_err(|error| fail(format!("the id {id} is not a seq: {error}")))?;
    ensure(seq == event.seq, || {
        format!("the id {seq} is not the event's seq {}", event.seq)
    })?;
    Ok((seq, event))
}
