use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use hmac::{Hmac, KeyInit, Mac};
use keel_types::{Event, Id};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::net::TcpListener;

use super::Suite;
use crate::client::{Answer, Credential};
use crate::report::{Aborted, Failure, ensure, fail};

pub const ADMIN_SECRET: &str = "whsec_conformance_admin_secret_01";
pub const USER_SECRET: &str = "whsec_conformance_user_secret_002";
pub const MASK: &str = "note.*";
pub const EVENT_SAVED: &str = "note.saved";
pub const SIGNATURE_HEADER: &str = "x-keel-signature";
pub const EVENT_ID_HEADER: &str = "x-keel-event-id";
pub const WINDOW_SECONDS: i64 = 300;
pub const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);
pub const QUIET: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct Received {
    pub hook: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn event(&self) -> Result<Event, Failure> {
        serde_json::from_slice(&self.body).map_err(|error| {
            fail(format!(
                "the delivery body is not an event: {error}: {}",
                String::from_utf8_lossy(&self.body)
            ))
        })
    }
}

#[derive(Clone, Default)]
struct Hooks {
    received: Arc<Mutex<Vec<Received>>>,
}

async fn hook(
    State(hooks): State<Hooks>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    hooks
        .received
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Received {
            hook: name,
            headers,
            body: body.to_vec(),
        });
    StatusCode::OK
}

pub struct Listener {
    port: u16,
    hooks: Hooks,
}

impl Listener {
    pub async fn start() -> Result<Self, Failure> {
        let hooks = Hooks::default();
        let router = Router::new()
            .route("/hook/{name}", post(hook))
            .with_state(hooks.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| fail(format!("the listener could not bind: {error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| fail(format!("the listener has no address: {error}")))?
            .port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { port, hooks })
    }

    pub fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/hook/{name}", self.port)
    }

    pub fn received(&self, name: &str) -> Vec<Received> {
        self.hooks
            .received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|received| received.hook == name)
            .cloned()
            .collect()
    }

    pub async fn wait_for_note(&self, name: &str, note: Id) -> Result<Received, Failure> {
        let started = Instant::now();
        loop {
            if let Some(hit) = self.received(name).into_iter().find(|hit| {
                hit.event()
                    .ok()
                    .is_some_and(|event| event.data["id"] == json!(note))
            }) {
                return Ok(hit);
            }
            ensure(started.elapsed() < DELIVERY_TIMEOUT, || {
                format!(
                    "no delivery for the note {note} reached /hook/{name} within {DELIVERY_TIMEOUT:?}"
                )
            })?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

pub fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn digest(secret: &[u8], t: i64, body: &[u8]) -> Result<Hmac<Sha256>, Failure> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| fail("the secret cannot key the signature"))?;
    mac.update(t.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    Ok(mac)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

pub fn parse_signature(header: &str) -> Result<(i64, Vec<u8>), Failure> {
    let mut t = None;
    let mut v1 = None;
    for part in header.split(',') {
        let (name, value) = part
            .trim()
            .split_once('=')
            .ok_or_else(|| fail(format!("the signature part {part} is not name=value")))?;
        match name {
            "t" => {
                t = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| fail(format!("t is not a number: {value}")))?,
                );
            }
            "v1" => v1 = Some(unhex(value).ok_or_else(|| fail("v1 is not hex"))?),
            _ => {}
        }
    }
    match (t, v1) {
        (Some(t), Some(v1)) => Ok((t, v1)),
        _ => Err(fail(format!("the signature lacks t or v1: {header}"))),
    }
}

pub fn verify(secret: &[u8], header: &str, body: &[u8], now: i64) -> Result<(), Failure> {
    let (t, v1) = parse_signature(header)?;
    ensure((now - t).abs() <= WINDOW_SECONDS, || {
        format!("the signature time {t} is more than {WINDOW_SECONDS} seconds from {now}")
    })?;
    digest(secret, t, body)?
        .verify_slice(&v1)
        .map_err(|_| fail("the signature does not match the body"))
}

pub async fn run(suite: &mut Suite) -> Result<(), Aborted> {
    let outcome = suite.login().await;
    suite
        .report
        .check("auth.login as the admin for the webhook checks", outcome)?;
    let listener = suite.report.check(
        "the suite opens a listener for webhook deliveries",
        Listener::start().await,
    )?;
    let answer = subscribe(
        suite,
        "conformance admin",
        &listener.url("admin"),
        ADMIN_SECRET,
    )
    .await;
    let admin_subscription = suite.report.check(
        "webhook.subscription.set as the admin to note.* answers the chosen secret once",
        subscribed(answer.map(Answer::confidential), ADMIN_SECRET),
    )?;
    let answer = edit_note(suite, "conformance-hook-admin").await;
    let admin_note = suite.report.check(
        "note.edit as the admin creates the note the webhook announces",
        note_id(answer),
    )?;
    let hit = suite.report.check(
        "the listener receives the delivery of note.saved within the timeout",
        listener.wait_for_note("admin", admin_note).await,
    )?;
    let now = now_seconds();
    suite.report.check(
        "the delivery carries Content-Type application/json and X-Keel-Event-Id equal to the body's id",
        shaped(&hit),
    )?;
    let header = hit.header(SIGNATURE_HEADER).unwrap_or_default().to_owned();
    suite.report.check(
        "X-Keel-Signature t=<seconds>,v1=<hex> verifies with HMAC-SHA256 over t.body under the secret",
        verify(ADMIN_SECRET.as_bytes(), &header, &hit.body, now),
    )?;
    let mut tampered = hit.body.clone();
    if let Some(byte) = tampered.last_mut() {
        *byte ^= 0x01;
    }
    suite.report.check(
        "a tampered body fails the verification",
        match verify(ADMIN_SECRET.as_bytes(), &header, &tampered, now) {
            Ok(()) => Err(fail("the tampered body verified")),
            Err(_) => Ok(()),
        },
    )?;
    let (t, _) = suite.report.check(
        "the signature time is within the window of the suite's clock",
        parse_signature(&header).and_then(|(t, v1)| {
            ensure((now - t).abs() <= WINDOW_SECONDS, || {
                format!("t {t} is far from now {now}")
            })?;
            Ok((t, v1))
        }),
    )?;
    suite.report.check(
        "a stale t outside the 300 second window fails the verification",
        match verify(
            ADMIN_SECRET.as_bytes(),
            &header,
            &hit.body,
            t + WINDOW_SECONDS + 1,
        ) {
            Ok(()) => Err(fail("the stale signature verified")),
            Err(_) => Ok(()),
        },
    )?;
    let answer = deliveries(suite, &admin_subscription).await;
    suite.report.check(
        "webhook.delivery.list scoped by the subscription shows the delivery as delivered with status 200",
        delivered(answer, admin_note),
    )?;

    let outcome = suite.find_user().await;
    let user = suite.report.check(
        "keel.principal.list finds the user for the webhook checks",
        outcome,
    )?;
    let outcome = suite.su(&user).await;
    suite
        .report
        .check("auth.su into the user for the webhook checks", outcome)?;
    let answer = subscribe(
        suite,
        "conformance user",
        &listener.url("user"),
        USER_SECRET,
    )
    .await;
    let user_subscription = suite.report.check(
        "webhook.subscription.set as the user to note.* answers the chosen secret once",
        subscribed(answer.map(Answer::confidential), USER_SECRET),
    )?;
    let answer = edit_note(suite, "conformance-hook-user").await;
    let user_note = suite.report.check(
        "note.edit as the user creates the user's note",
        note_id(answer),
    )?;
    let outcome = suite.su_exit().await;
    suite
        .report
        .check("auth.su.exit back to the admin", outcome)?;
    let answer = edit_note(suite, "conformance-hook-admin-2").await;
    let second_admin_note = suite.report.check(
        "note.edit as the admin creates a second note of the admin",
        note_id(answer),
    )?;
    let hit = suite.report.check(
        "the user's subscription receives the user's note.saved",
        listener.wait_for_note("user", user_note).await,
    )?;
    let header = hit.header(SIGNATURE_HEADER).unwrap_or_default().to_owned();
    suite.report.check(
        "the user's delivery verifies under the user's secret",
        verify(USER_SECRET.as_bytes(), &header, &hit.body, now_seconds()),
    )?;
    let both = match listener.wait_for_note("admin", user_note).await {
        Ok(_) => listener
            .wait_for_note("admin", second_admin_note)
            .await
            .map(|_| ()),
        Err(failure) => Err(failure),
    };
    suite
        .report
        .check("the admin's subscription receives both notes", both)?;
    tokio::time::sleep(QUIET).await;
    suite.report.check(
        "the user's subscription receives the user's note alone",
        only_the_users(&listener, user_note),
    )?;
    for (name, id) in [("admin", &admin_subscription), ("user", &user_subscription)] {
        let answer = suite
            .client
            .call(
                "webhook.subscription.delete",
                json!({ "id": id }),
                Credential::Session,
                &[],
            )
            .await;
        suite.report.check(
            &format!("webhook.subscription.delete removes the {name} subscription"),
            answer.and_then(|answer| {
                let (data, _) = answer.data::<Value>()?;
                ensure(data["deleted"] == json!(true), || {
                    answer.failure("data.deleted is not true").0
                })
            }),
        )?;
    }
    let outcome = suite.logout().await;
    suite
        .report
        .check("auth.logout after the webhook checks", outcome)?;
    Ok(())
}

async fn subscribe(
    suite: &mut Suite,
    name: &str,
    url: &str,
    secret: &str,
) -> Result<Answer, Failure> {
    suite
        .client
        .call(
            "webhook.subscription.set",
            json!({ "name": name, "url": url, "secret": secret, "events": [MASK] }),
            Credential::Session,
            &[],
        )
        .await
}

async fn edit_note(suite: &mut Suite, title: &str) -> Result<Answer, Failure> {
    suite
        .client
        .call(
            "note.edit",
            json!({ "title": format!("{title}-{}", Id::generate()) }),
            Credential::Session,
            &[],
        )
        .await
}

async fn deliveries(suite: &mut Suite, subscription: &Id) -> Result<Answer, Failure> {
    let started = Instant::now();
    loop {
        let answer = suite
            .client
            .call(
                "webhook.delivery.list",
                json!({ "scope": { "subscription_id": subscription } }),
                Credential::Session,
                &[],
            )
            .await?;
        let (page, _) = answer.data::<Value>()?;
        let settled = page["data"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["state"] == json!("delivered")));
        if settled || started.elapsed() >= DELIVERY_TIMEOUT {
            return Ok(answer);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn subscribed(answer: Result<Answer, Failure>, secret: &str) -> Result<Id, Failure> {
    let answer = answer?;
    let (data, _) = answer.data::<Value>()?;
    ensure(data["secret"] == json!(secret), || {
        answer.failure("data.secret is not the chosen secret").0
    })?;
    ensure(data["enabled"] == json!(true), || {
        answer.failure("data.enabled is not true").0
    })?;
    ensure(data["events"] == json!([MASK]), || {
        answer.failure("data.events differ from the masks").0
    })?;
    super::id_of(&data, "id").map_err(|failure| answer.failure(failure.0))
}

fn note_id(answer: Result<Answer, Failure>) -> Result<Id, Failure> {
    let answer = answer?;
    let (row, _) = answer.data::<Value>()?;
    super::id_of(&row, "id").map_err(|failure| answer.failure(failure.0))
}

fn shaped(hit: &Received) -> Result<(), Failure> {
    let event = hit.event()?;
    ensure(
        hit.header("content-type")
            .is_some_and(|value| value.starts_with("application/json")),
        || format!("Content-Type is {:?}", hit.header("content-type")),
    )?;
    ensure(
        hit.header(EVENT_ID_HEADER) == Some(event.id.to_string().as_str()),
        || {
            format!(
                "{EVENT_ID_HEADER} is {:?} while the body's id is {}",
                hit.header(EVENT_ID_HEADER),
                event.id
            )
        },
    )?;
    ensure(event.kind == EVENT_SAVED, || {
        format!("the event type is {}", event.kind)
    })?;
    ensure(event.module == "note", || {
        format!("the event module is {}", event.module)
    })?;
    ensure(event.subject.is_some(), || {
        "the event has no subject".to_owned()
    })
}

fn delivered(answer: Result<Answer, Failure>, note: Id) -> Result<(), Failure> {
    let answer = answer?;
    let (page, _) = answer.data::<Value>()?;
    let rows = page["data"]
        .as_array()
        .ok_or_else(|| answer.failure("no data array"))?;
    ensure(!rows.is_empty(), || {
        answer.failure("no delivery row for the subscription").0
    })?;
    let row = rows
        .iter()
        .find(|row| row["state"] == json!("delivered"))
        .ok_or_else(|| answer.failure(format!("no delivered row for the note {note}")))?;
    ensure(row["lastStatus"] == json!(200), || {
        answer.failure("lastStatus is not 200").0
    })?;
    ensure(row["attempt"] == json!(1), || {
        answer.failure("attempt is not 1").0
    })?;
    ensure(row["type"] == json!(EVENT_SAVED), || {
        answer.failure("type is not note.saved").0
    })
}

fn only_the_users(listener: &Listener, user_note: Id) -> Result<(), Failure> {
    let notes: Vec<Value> = listener
        .received("user")
        .iter()
        .filter_map(|hit| hit.event().ok())
        .map(|event| event.data["id"].clone())
        .collect();
    ensure(notes == [json!(user_note)], || {
        format!("the user's hook received the notes {notes:?}, expected {user_note} alone")
    })
}
