use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::serde::Serialize;
use keel_sdk::serde_json;
use keel_sdk::system::PluginProcess;
use keel_sdk::types::{CallError, ErrorCode, Principal};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::config::Timings;

pub const ERROR_ANSWER: &str = "plugin.error.answer";
pub const ERROR_FRAME: &str = "plugin.error.frame";
pub const ERROR_INVOKE: &str = "plugin.error.invoke";
pub const ERROR_TOO_LARGE: &str = "plugin.error.too_large";
const FRAME_LIMIT: usize = 1024 * 1024;
const PENDING_LIMIT: usize = 64;
const LIVENESS: &str = "ping";
pub(super) const CALL: &str = "call";
pub(super) const BEFORE: &str = "before";
const INVOKE: &str = "invoke";
const INBOX: usize = 16;
const CLOSED_NAMESPACE: &str = "auth";
const REFUSAL_CODES: [(&str, ErrorCode); 5] = [
    ("validation", ErrorCode::Validation),
    ("forbidden", ErrorCode::Forbidden),
    ("not_found", ErrorCode::NotFound),
    ("conflict", ErrorCode::Conflict),
    ("unavailable", ErrorCode::Unavailable),
];

type Pending = Arc<Mutex<BTreeMap<String, Waiter>>>;
type Writer = Arc<tokio::sync::Mutex<OwnedWriteHalf>>;

enum Waiter {
    Liveness(oneshot::Sender<Value>),
    Handled(mpsc::Sender<Value>),
}

struct Entry {
    pending: Pending,
    id: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Timeout,
    Stuck,
    Closed,
    Busy,
    TooLarge,
    Start(String),
}

pub(super) struct Request<'a> {
    pub(super) t: &'static str,
    pub(super) func: Option<&'a str>,
    pub(super) principal: Option<&'a Principal>,
    pub(super) params: Option<&'a Value>,
}

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde")]
struct Outgoing<'a> {
    id: &'a str,
    t: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    func: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    principal: Option<&'a Principal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
}

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde")]
struct Answer<'a> {
    id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a CallError>,
}

pub(super) struct Scope {
    plugin: String,
    level: Level,
    levels: BTreeMap<String, Level>,
}

pub(super) struct Invoking<'a> {
    pub(super) call: &'a Call<Value>,
    pub(super) scope: &'a Scope,
}

pub(super) struct Connection {
    plugin: String,
    writer: Writer,
    pending: Pending,
    closed: Arc<watch::Sender<bool>>,
    reader: JoinHandle<()>,
    next: AtomicU64,
}

impl Fault {
    pub(super) fn reason(&self, timings: &Timings) -> String {
        match self {
            Fault::Timeout => format!(
                "a frame got no answer within {} ms",
                timings.timeout.as_millis()
            ),
            Fault::Stuck => format!("it read no frame within {} ms", timings.timeout.as_millis()),
            Fault::Closed => "its process closed its output before it answered".to_owned(),
            Fault::Busy => format!("it holds {PENDING_LIMIT} frames without an answer"),
            Fault::TooLarge => format!("a frame for it exceeds {FRAME_LIMIT} bytes"),
            Fault::Start(detail) => format!("its process could not start: {detail}"),
        }
    }
}

impl Scope {
    pub(super) fn new(plugin: &str, level: Level, levels: BTreeMap<String, Level>) -> Self {
        Self {
            plugin: plugin.to_owned(),
            level,
            levels,
        }
    }

    fn admit(&self, func: &str, principal: &Principal) -> Result<()> {
        let within = func.split('.').next() != Some(CLOSED_NAMESPACE)
            && self
                .levels
                .get(func)
                .is_some_and(|level| *level <= self.level)
            && principal.scope.admits(func);
        if within {
            return Ok(());
        }
        Err(KeelError::forbidden(ERROR_INVOKE)
            .param("plugin", self.plugin.as_str())
            .param("function", func))
    }
}

impl Invoking<'_> {
    async fn answer(&self, frame: &Value) -> Result<Value> {
        let func = frame.get("func").and_then(Value::as_str);
        let params = match frame.get("params") {
            None | Some(Value::Null) => Some(json!({})),
            Some(params @ Value::Object(_)) => Some(params.clone()),
            Some(_) => None,
        };
        let (Some(func), Some(params)) = (func, params) else {
            return Err(frame_error(&self.scope.plugin));
        };
        self.scope.admit(func, self.call.principal())?;
        self.call.invoke(func, params).await
    }
}

impl Connection {
    pub(super) fn open(
        plugin: &str,
        process: PluginProcess,
        timeout: Duration,
    ) -> io::Result<Self> {
        process.stdio.set_nonblocking(true)?;
        let (read, write) = UnixStream::from_std(process.stdio)?.into_split();
        let writer: Writer = Arc::new(tokio::sync::Mutex::new(write));
        let pending = Pending::default();
        let closed = Arc::new(watch::Sender::new(false));
        let reader = tokio::spawn(read_frames(Reader {
            plugin: plugin.to_owned(),
            read,
            writer: Arc::clone(&writer),
            pending: Arc::clone(&pending),
            closed: Arc::clone(&closed),
            timeout,
        }));
        Ok(Self {
            plugin: plugin.to_owned(),
            writer,
            pending,
            closed,
            reader,
            next: AtomicU64::new(0),
        })
    }

    pub(super) async fn closed(&self) {
        let mut closed = self.closed.subscribe();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    pub(super) async fn ping(&self, timeout: Duration) -> std::result::Result<Value, Fault> {
        let request = Request {
            t: LIVENESS,
            func: None,
            principal: None,
            params: None,
        };
        let (sender, receiver) = oneshot::channel();
        let (entry, deadline) = self
            .send(&request, Waiter::Liveness(sender), timeout)
            .await?;
        let answer = tokio::time::timeout_at(deadline, receiver).await;
        drop(entry);
        match answer {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err(Fault::Closed),
            Err(_) => Err(Fault::Timeout),
        }
    }

    pub(super) async fn handle(
        &self,
        request: &Request<'_>,
        timeout: Duration,
        invoking: &Invoking<'_>,
    ) -> std::result::Result<Value, Fault> {
        let (sender, mut inbox) = mpsc::channel(INBOX);
        let (entry, deadline) = self.send(request, Waiter::Handled(sender), timeout).await?;
        let exchanged = tokio::time::timeout_at(deadline, async {
            while let Some(frame) = inbox.recv().await {
                let reply = match frame.get("t").and_then(Value::as_str) {
                    None if frame.get("t").is_none() => return Ok(frame),
                    Some(INVOKE) => invoking.answer(&frame).await,
                    _ => Err(frame_error(&self.plugin)),
                };
                self.reply(&entry.id, reply, deadline).await?;
            }
            Err(Fault::Closed)
        })
        .await;
        drop(entry);
        exchanged.unwrap_or(Err(Fault::Timeout))
    }

    async fn send(
        &self,
        request: &Request<'_>,
        waiter: Waiter,
        timeout: Duration,
    ) -> std::result::Result<(Entry, Instant), Fault> {
        if *self.closed.borrow() {
            return Err(Fault::Closed);
        }
        let id = format!("k-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let line = line(&Outgoing {
            id: &id,
            t: request.t,
            func: request.func,
            principal: request.principal,
            params: request.params,
        })
        .ok_or(Fault::TooLarge)?;
        {
            let mut pending = lock(&self.pending);
            if pending.len() >= PENDING_LIMIT {
                return Err(Fault::Busy);
            }
            pending.insert(id.clone(), waiter);
        }
        let entry = Entry {
            pending: Arc::clone(&self.pending),
            id,
        };
        let deadline = Instant::now() + timeout;
        write(&self.writer, &self.closed, line, deadline).await?;
        Ok((entry, deadline))
    }

    async fn reply(
        &self,
        id: &str,
        reply: Result<Value>,
        deadline: Instant,
    ) -> std::result::Result<(), Fault> {
        let line = answer_line(id, reply, &self.plugin);
        write(&self.writer, &self.closed, line, deadline).await
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        lock(&self.pending).remove(&self.id);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

struct Reader {
    plugin: String,
    read: OwnedReadHalf,
    writer: Writer,
    pending: Pending,
    closed: Arc<watch::Sender<bool>>,
    timeout: Duration,
}

async fn read_frames(reader: Reader) {
    let Reader {
        plugin,
        read,
        writer,
        pending,
        closed,
        timeout,
    } = reader;
    let mut read = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut limited = (&mut read).take(FRAME_LIMIT as u64 + 1);
        match limited.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            if line.len() > FRAME_LIMIT {
                tracing::warn!(
                    "a plugin wrote a frame over {FRAME_LIMIT} bytes; its output is closed"
                );
            }
            break;
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(id) = frame.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let unanswered = {
            let mut pending = lock(&pending);
            match pending.get(&id) {
                Some(Waiter::Handled(inbox)) => {
                    if let Err(TrySendError::Closed(_)) = inbox.try_send(frame) {
                        pending.remove(&id);
                    }
                    None
                }
                Some(Waiter::Liveness(_)) => {
                    if let Some(Waiter::Liveness(waiting)) = pending.remove(&id) {
                        let _ = waiting.send(frame);
                    }
                    None
                }
                None => frame.get("t").is_some().then_some(id),
            }
        };
        let Some(id) = unanswered else {
            continue;
        };
        let line = answer_line(&id, Err(frame_error(&plugin)), &plugin);
        if write(&writer, &closed, line, Instant::now() + timeout)
            .await
            .is_err()
        {
            break;
        }
    }
    lock(&pending).clear();
    closed.send_replace(true);
}

async fn write(
    writer: &Writer,
    closed: &Arc<watch::Sender<bool>>,
    line: Vec<u8>,
    deadline: Instant,
) -> std::result::Result<(), Fault> {
    let writer = Arc::clone(writer);
    let closed = Arc::clone(closed);
    let written = tokio::spawn(async move {
        let written = tokio::time::timeout_at(deadline, async {
            let mut writer = writer.lock().await;
            writer.write_all(&line).await?;
            writer.flush().await
        })
        .await;
        let fault = match written {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(_)) => Fault::Closed,
            Err(_) => Fault::Stuck,
        };
        closed.send_replace(true);
        Err(fault)
    });
    written.await.unwrap_or(Err(Fault::Closed))
}

fn line(frame: &impl Serialize) -> Option<Vec<u8>> {
    let mut line = serde_json::to_vec(frame).ok()?;
    if line.len() > FRAME_LIMIT {
        return None;
    }
    line.push(b'\n');
    Some(line)
}

fn answer_line(id: &str, reply: Result<Value>, plugin: &str) -> Vec<u8> {
    let encoded = match &reply {
        Ok(data) => line(&Answer {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }),
        Err(error) => line(&Answer {
            id,
            ok: false,
            data: None,
            error: Some(&error.to_call_error()),
        }),
    };
    encoded.unwrap_or_else(|| {
        let error = too_large(plugin).to_call_error();
        line(&Answer {
            id,
            ok: false,
            data: None,
            error: Some(&error),
        })
        .unwrap_or_default()
    })
}

pub(super) fn answered(answer: &Value, plugin: &str) -> Result<Value> {
    match answer.get("ok") {
        Some(Value::Bool(true)) => Ok(answer.get("data").cloned().unwrap_or(Value::Null)),
        Some(Value::Bool(false)) => Err(refusal(answer.get("error"), plugin, false)),
        _ => Err(malformed(plugin)),
    }
}

pub(super) fn verdict(answer: &Value, plugin: &str) -> Result<Before> {
    match answer.get("ok") {
        Some(Value::Bool(true)) => match answer.get("params") {
            None | Some(Value::Null) => Ok(Before::Proceed),
            Some(params @ Value::Object(_)) => Ok(Before::Enrich(params.clone())),
            Some(_) => Err(malformed(plugin)),
        },
        Some(Value::Bool(false)) => Err(refusal(answer.get("error"), plugin, true)),
        _ => Err(malformed(plugin)),
    }
}

fn refusal(error: Option<&Value>, plugin: &str, veto: bool) -> KeelError {
    let Some(Value::Object(error)) = error else {
        return malformed(plugin);
    };
    let prefix = format!("{plugin}.");
    let Some(key) = error
        .get("key")
        .and_then(Value::as_str)
        .filter(|key| key.len() > prefix.len() && key.starts_with(&prefix))
    else {
        return malformed(plugin);
    };
    let code = match (veto, error.get("code")) {
        (true, _) | (false, None | Some(Value::Null)) => Some(ErrorCode::Validation),
        (false, Some(Value::String(code))) => REFUSAL_CODES
            .iter()
            .find(|(name, _)| *name == code.as_str())
            .map(|(_, code)| *code),
        (false, Some(_)) => None,
    };
    let Some(code) = code else {
        return malformed(plugin);
    };
    let mut refused = KeelError::with_key(code, key);
    match error.get("params") {
        None | Some(Value::Null) => {}
        Some(Value::Object(params)) => {
            for (name, value) in params {
                refused = refused.param(name.as_str(), value.clone());
            }
        }
        Some(_) => return malformed(plugin),
    }
    match error.get("field") {
        None | Some(Value::Null) => refused,
        Some(Value::String(path)) if !path.is_empty() => refused.field(path.as_str()),
        Some(_) => malformed(plugin),
    }
}

pub(super) fn malformed(plugin: &str) -> KeelError {
    KeelError::internal(ERROR_ANSWER).param("plugin", plugin)
}

pub(super) fn too_large(plugin: &str) -> KeelError {
    KeelError::validation(ERROR_TOO_LARGE)
        .param("plugin", plugin)
        .param("limit", FRAME_LIMIT)
}

fn frame_error(plugin: &str) -> KeelError {
    KeelError::validation(ERROR_FRAME).param("plugin", plugin)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream as Socket;

    use super::*;
    use crate::plugins::config::PluginConfig;

    fn connection(kernel: Socket) -> Connection {
        Connection::open(
            "acme",
            PluginProcess {
                pid: 0,
                stdio: kernel,
            },
            Duration::from_secs(5),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_plugin_that_reads_no_frame_leaves_the_write_stuck_until_the_timeout() {
        let (kernel, _plugin) = Socket::pair().unwrap();
        let connection = connection(kernel);
        let params = json!({ "pad": "x".repeat(FRAME_LIMIT / 2) });
        let request = Request {
            t: CALL,
            func: Some("acme.sync.list"),
            principal: None,
            params: Some(&params),
        };
        let call = keel_sdk::runtime::CallBuilder::new(principal(), "req-1")
            .read(keel_sdk::sea_orm::DatabaseConnection::Disconnected)
            .call(json!({}));
        let scope = Scope::new("acme", Level::User, BTreeMap::new());
        let invoking = Invoking {
            call: &call,
            scope: &scope,
        };
        let timeout = Duration::from_millis(200);
        let mut faults = Vec::new();
        while faults.len() < 16 && faults.last() != Some(&Fault::Stuck) {
            faults.push(
                connection
                    .handle(&request, timeout, &invoking)
                    .await
                    .unwrap_err(),
            );
        }
        let fault = faults.pop().expect("a frame was sent");
        assert_eq!(fault, Fault::Stuck, "{faults:?}");
        assert!(
            faults.iter().all(|fault| *fault == Fault::Timeout),
            "{faults:?}"
        );
        let timings = Timings {
            timeout,
            ..PluginConfig::default().timings()
        };
        assert_eq!(fault.reason(&timings), "it read no frame within 200 ms");
        assert!(*connection.closed.borrow());
        assert!(lock(&connection.pending).is_empty());
    }

    #[tokio::test]
    async fn a_frame_over_the_limit_is_never_written() {
        let (kernel, _plugin) = Socket::pair().unwrap();
        let connection = connection(kernel);
        let params = json!({ "pad": "x".repeat(FRAME_LIMIT) });
        let request = Request {
            t: CALL,
            func: Some("acme.sync.list"),
            principal: None,
            params: Some(&params),
        };
        let call = keel_sdk::runtime::CallBuilder::new(principal(), "req-1")
            .read(keel_sdk::sea_orm::DatabaseConnection::Disconnected)
            .call(json!({}));
        let scope = Scope::new("acme", Level::User, BTreeMap::new());
        let invoking = Invoking {
            call: &call,
            scope: &scope,
        };
        let fault = connection
            .handle(&request, Duration::from_secs(1), &invoking)
            .await
            .unwrap_err();
        assert_eq!(fault, Fault::TooLarge);
        assert!(!*connection.closed.borrow());
        assert!(lock(&connection.pending).is_empty());
    }

    #[tokio::test]
    async fn frames_whose_callers_went_away_leave_no_place_taken_and_a_later_frame_is_answered() {
        let (kernel, plugin) = Socket::pair().unwrap();
        let connection = connection(kernel);
        plugin.set_nonblocking(true).unwrap();
        let (read, mut write) = UnixStream::from_std(plugin).unwrap().into_split();
        let mut lines = BufReader::new(read).lines();
        let request = Request {
            t: CALL,
            func: Some("acme.sync.list"),
            principal: None,
            params: None,
        };
        let call = keel_sdk::runtime::CallBuilder::new(principal(), "req-1")
            .read(keel_sdk::sea_orm::DatabaseConnection::Disconnected)
            .call(json!({}));
        let scope = Scope::new("acme", Level::User, BTreeMap::new());
        let invoking = Invoking {
            call: &call,
            scope: &scope,
        };
        let timeout = Duration::from_secs(5);
        for _ in 0..PENDING_LIMIT {
            let abandoned = tokio::time::timeout(
                Duration::from_millis(20),
                connection.handle(&request, timeout, &invoking),
            )
            .await;
            assert!(abandoned.is_err());
            assert!(lines.next_line().await.unwrap().is_some());
        }
        assert!(lock(&connection.pending).is_empty());
        let (sender, inbox) = mpsc::channel(INBOX);
        drop(inbox);
        lock(&connection.pending).insert("k-gone".to_owned(), Waiter::Handled(sender));
        write
            .write_all(b"{\"id\":\"k-gone\",\"ok\":true,\"result\":0}\n")
            .await
            .unwrap();
        let answering = async {
            let line = lines.next_line().await.unwrap().unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            let answer = json!({ "id": frame["id"], "ok": true, "result": 7 });
            write
                .write_all(format!("{answer}\n").as_bytes())
                .await
                .unwrap();
        };
        let (answer, ()) = tokio::join!(connection.handle(&request, timeout, &invoking), answering);
        assert_eq!(answer.unwrap()["result"], json!(7));
        assert!(lock(&connection.pending).is_empty());
        assert!(!*connection.closed.borrow());
    }

    #[test]
    fn an_answer_too_large_for_a_frame_becomes_the_too_large_error() {
        let line = answer_line("k-1", Ok(json!("x".repeat(FRAME_LIMIT))), "acme");
        let answer: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(answer["id"], json!("k-1"));
        assert_eq!(answer["ok"], json!(false));
        assert_eq!(answer["error"]["key"], json!(ERROR_TOO_LARGE));
        assert_eq!(answer["error"]["params"]["limit"], json!(FRAME_LIMIT));
    }

    #[test]
    fn a_refusal_keeps_its_key_params_field_and_allowed_code_inside_the_plugin_prefix() {
        let refused = answered(
            &json!({ "id": "k-1", "ok": false, "error": { "code": "not_found", "key": "acme.error.gone", "params": { "name": "a.org" } } }),
            "acme",
        )
        .unwrap_err();
        assert_eq!(refused.code(), ErrorCode::NotFound);
        assert_eq!(refused.key(), "acme.error.gone");
        assert_eq!(refused.params()["name"], json!("a.org"));
        let vetoed = verdict(
            &json!({ "id": "k-2", "ok": false, "error": { "code": "internal", "key": "acme.error.linked", "field": "ids" } }),
            "acme",
        )
        .unwrap_err();
        let error = vetoed.to_call_error();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.fields[0].path, "ids");
        assert_eq!(error.fields[0].key.as_deref(), Some("acme.error.linked"));
        for answer in [
            json!({ "ok": "yes" }),
            json!({ "ok": false }),
            json!({ "ok": false, "error": { "key": "keel.error.unauthenticated" } }),
            json!({ "ok": false, "error": { "key": "acme." } }),
            json!({ "ok": false, "error": { "key": "acme.error.x", "code": "unauthenticated" } }),
            json!({ "ok": false, "error": { "key": "acme.error.x", "params": ["a"] } }),
            json!({ "ok": false, "error": { "key": "acme.error.x", "field": 3 } }),
        ] {
            assert_eq!(
                answered(&answer, "acme").unwrap_err().key(),
                ERROR_ANSWER,
                "{answer}"
            );
        }
        assert_eq!(
            verdict(&json!({ "ok": true, "params": [1] }), "acme")
                .unwrap_err()
                .key(),
            ERROR_ANSWER
        );
        assert_eq!(
            verdict(&json!({ "ok": true, "params": { "note": 1 } }), "acme").unwrap(),
            Before::Enrich(json!({ "note": 1 }))
        );
        assert_eq!(
            verdict(&json!({ "ok": true }), "acme").unwrap(),
            Before::Proceed
        );
        assert_eq!(
            answered(&json!({ "ok": true }), "acme").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn the_scope_of_a_plugin_admits_known_functions_up_to_its_level_outside_auth_and_the_callers_masks()
     {
        let levels = BTreeMap::from([
            ("dns.zone.get".to_owned(), Level::User),
            ("dns.server.restart".to_owned(), Level::Admin),
            ("auth.token.list".to_owned(), Level::User),
        ]);
        let scope = Scope::new("acme", Level::User, levels);
        let mut caller = principal();
        caller.level = Level::Admin;
        scope.admit("dns.zone.get", &caller).unwrap();
        for refused in ["dns.server.restart", "auth.token.list", "dns.zone.purge"] {
            let error = scope.admit(refused, &caller).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Forbidden);
            assert_eq!(error.key(), ERROR_INVOKE);
            assert_eq!(error.params()["function"], json!(refused));
        }
        caller.scope = keel_sdk::types::Scope::Functions(vec!["acme.*".to_owned()]);
        assert_eq!(
            scope.admit("dns.zone.get", &caller).unwrap_err().key(),
            ERROR_INVOKE
        );
    }

    fn principal() -> Principal {
        Principal {
            id: Id::generate(),
            login: "alice".to_owned(),
            level: Level::User,
            account_id: Id::generate(),
            role: "user".to_owned(),
            actor: None,
            scope: keel_sdk::types::Scope::Full,
            via: keel_sdk::types::Via::Http,
            external_id: None,
            origin: None,
            lang: "en".to_owned(),
            grants: BTreeMap::new(),
        }
    }
}
