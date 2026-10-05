use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use keel_types::{Envelope, Event, Id, Meta};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::neighbour::Identity;
use crate::report::{Failure, ensure, fail};

pub const EVENT_ID_HEADER: &str = "x-keel-event-id";
pub const PEER_HEADER: &str = "x-keel-peer";
pub const HOOK_PATH: &str = "/api/v1/x/peer/hook";
pub const SECRET_MEMBER: &str = "webhookSecret";
pub const WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct Received {
    pub hook: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    pub status: u16,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn event(&self) -> Result<Event, Failure> {
        serde_json::from_slice(&self.body).map_err(|error| {
            fail(format!(
                "the delivery body is not an event: {error}: {}",
                String::from_utf8_lossy(&self.body)
            ))
        })
    }

    pub fn about(&self, subject: Id) -> bool {
        self.event().ok().is_some_and(|event| {
            event
                .subject
                .as_ref()
                .is_some_and(|about| about.id == subject)
        })
    }
}

#[derive(Clone, Debug)]
pub struct Neighbour {
    pub identity: Identity,
    pub answer: Value,
}

#[derive(Default)]
struct Shared {
    received: Mutex<Vec<Received>>,
    failed: Mutex<HashSet<(String, String)>>,
    neighbours: Mutex<HashMap<String, Neighbour>>,
    pairings: Mutex<Vec<(String, Value)>>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn record(&self, hook: String, headers: HeaderMap, body: Bytes, status: StatusCode) {
        locked(&self.received).push(Received {
            hook,
            headers,
            body: body.to_vec(),
            status: status.as_u16(),
        });
    }
}

async fn hook(
    State(shared): State<Arc<Shared>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    shared.record(format!("hook/{name}"), headers, body, StatusCode::OK);
    StatusCode::OK
}

async fn flaky(
    State(shared): State<Arc<Shared>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let event = headers
        .get(EVENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let first = locked(&shared.failed).insert((name.clone(), event));
    let status = if first {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    };
    shared.record(format!("flaky/{name}"), headers, body, status);
    status
}

async fn peer_hook(
    State(shared): State<Arc<Shared>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    shared.record(format!("peer/{name}"), headers, body, StatusCode::OK);
    StatusCode::OK
}

async fn handshake(State(shared): State<Arc<Shared>>, Path(name): Path<String>) -> Response {
    match locked(&shared.neighbours).get(&name) {
        Some(neighbour) => Json(neighbour.identity.instance()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn pair(
    State(shared): State<Arc<Shared>>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let answer = locked(&shared.neighbours)
        .get(&name)
        .map(|neighbour| answered(&neighbour.answer, &body));
    locked(&shared.pairings).push((name, body));
    match answer {
        Some(answer) => Json(Envelope::ok(
            answer,
            Meta {
                request_id: format!("conformance-{}", Id::generate()),
                principal: None,
            },
        ))
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn answered(answer: &Value, request: &Value) -> Value {
    let reachable = request["url"].is_string();
    let subscribed = request["subscribe"]
        .as_array()
        .is_some_and(|masks| !masks.is_empty());
    let mut answer = answer.clone();
    if !(reachable && subscribed)
        && let Some(object) = answer.as_object_mut()
    {
        object.remove(SECRET_MEMBER);
    }
    answer
}

pub struct Listener {
    port: u16,
    shared: Arc<Shared>,
}

impl Listener {
    pub async fn start() -> Result<Self, Failure> {
        let shared = Arc::new(Shared::default());
        let router = Router::new()
            .route("/hook/{name}", post(hook))
            .route("/flaky/{name}", post(flaky))
            .route("/peer/{name}/api/v1/x/peer/hook", post(peer_hook))
            .route("/peer/{name}/api/v1/instance", get(handshake))
            .route("/peer/{name}/api/v1/call/peer.pair", post(pair))
            .with_state(shared.clone());
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
        Ok(Self { port, shared })
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn hook_url(&self, name: &str) -> String {
        format!("{}/hook/{name}", self.base())
    }

    pub fn flaky_url(&self, name: &str) -> String {
        format!("{}/flaky/{name}", self.base())
    }

    pub fn peer_url(&self, name: &str) -> String {
        format!("{}/peer/{name}", self.base())
    }

    pub fn neighbour(&self, name: &str, neighbour: Neighbour) {
        locked(&self.shared.neighbours).insert(name.to_owned(), neighbour);
    }

    pub fn pairings(&self, name: &str) -> Vec<Value> {
        locked(&self.shared.pairings)
            .iter()
            .filter(|(neighbour, _)| neighbour == name)
            .map(|(_, body)| body.clone())
            .collect()
    }

    pub fn received(&self, hook: &str) -> Vec<Received> {
        locked(&self.shared.received)
            .iter()
            .filter(|received| received.hook == hook)
            .cloned()
            .collect()
    }

    pub async fn wait_for(
        &self,
        hook: &str,
        what: &str,
        mut wanted: impl FnMut(&Received) -> bool,
    ) -> Result<Received, Failure> {
        let started = Instant::now();
        loop {
            if let Some(hit) = self.received(hook).into_iter().find(&mut wanted) {
                return Ok(hit);
            }
            ensure(started.elapsed() < WAIT, || {
                format!("no delivery of {what} reached /{hook} within {WAIT:?}")
            })?;
            tokio::time::sleep(POLL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_client::{Request, Trust};
    use keel_types::InstanceInfo;
    use serde_json::json;

    use super::*;

    fn client(listener: &Listener) -> keel_client::Client {
        keel_client::Client::builder(&listener.base())
            .trust(Trust::System)
            .build()
            .expect("a client of the listener")
    }

    #[tokio::test]
    async fn the_flaky_hook_fails_the_first_attempt_of_each_event_and_admits_the_next() {
        let listener = Listener::start().await.unwrap();
        let client = client(&listener);
        let send = |event: &str| {
            Request::post("/flaky/retry", &json!({ "id": event }))
                .header(axum::http::HeaderName::from_static(EVENT_ID_HEADER), event)
        };
        let statuses = [
            client.send(send("one")).await.unwrap().status,
            client.send(send("one")).await.unwrap().status,
            client.send(send("two")).await.unwrap().status,
            client.send(send("one")).await.unwrap().status,
        ];
        assert_eq!(statuses, [500, 200, 500, 200]);
        let recorded: Vec<u16> = listener
            .received("flaky/retry")
            .iter()
            .map(|hit| hit.status)
            .collect();
        assert_eq!(recorded, [500, 200, 500, 200]);
        assert!(listener.received("hook/retry").is_empty());
    }

    #[test]
    fn the_neighbour_issues_a_webhook_secret_only_to_an_initiator_with_an_address_and_masks() {
        let answer = json!({ "peerId": "B", "webhookSecret": "whs_1" });
        let full = json!({ "url": "https://a.example", "subscribe": ["conformance.*"] });
        assert_eq!(answered(&answer, &full), answer);
        for request in [
            json!({ "subscribe": ["conformance.*"] }),
            json!({ "url": null, "subscribe": ["conformance.*"] }),
            json!({ "url": "https://a.example", "subscribe": [] }),
            json!({ "url": "https://a.example" }),
        ] {
            assert_eq!(
                answered(&answer, &request),
                json!({ "peerId": "B" }),
                "{request}"
            );
        }
    }

    #[tokio::test]
    async fn a_registered_neighbour_answers_its_handshake_and_records_the_pairing() {
        let listener = Listener::start().await.unwrap();
        let client = client(&listener);
        let missing = client
            .send(Request::get("/peer/b/api/v1/instance"))
            .await
            .unwrap();
        assert_eq!(missing.status, 404);
        let identity = Identity::new(Id::generate(), "0.3.0").unwrap();
        let instance = identity.instance();
        listener.neighbour(
            "b",
            Neighbour {
                identity,
                answer: json!({ "peerId": instance.instance_id }),
            },
        );
        let handshake = client
            .send(Request::get("/peer/b/api/v1/instance"))
            .await
            .unwrap();
        assert_eq!(handshake.json::<InstanceInfo>().unwrap(), instance);
        let answered = client
            .send(Request::post(
                "/peer/b/api/v1/call/peer.pair",
                &json!({ "code": "PAIR-AAAA-BBBB" }),
            ))
            .await
            .unwrap();
        let answer = answered.answer::<Value>().unwrap();
        assert_eq!(answer.data["peerId"], json!(instance.instance_id));
        assert_eq!(
            listener.pairings("b"),
            [json!({ "code": "PAIR-AAAA-BBBB" })]
        );
        assert!(listener.pairings("c").is_empty());
    }
}
