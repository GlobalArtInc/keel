use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{EntityTrait, Set};
use keel_sdk::system::{AuditEvent, AuditOutcome, AuditSink};
use keel_sdk::types::Via;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::entity::call;

pub const CHANNEL_CAPACITY: usize = 10_000;
pub const BATCH_ROWS: usize = 200;
pub const BATCH_WAIT: Duration = Duration::from_millis(250);
pub const STATUS_COMMITTED: &str = "committed";
pub const STATUS_FAILED: &str = "failed";

enum Message {
    Event(Box<AuditEvent>),
    Flush(oneshot::Sender<()>),
    Stop(oneshot::Sender<()>),
}

pub struct Sink {
    tx: mpsc::Sender<Message>,
    dropped: AtomicU64,
}

pub struct Worker {
    rx: mpsc::Receiver<Message>,
}

pub fn channel() -> (Arc<Sink>, Worker) {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    (
        Arc::new(Sink {
            tx,
            dropped: AtomicU64::new(0),
        }),
        Worker { rx },
    )
}

impl Sink {
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub async fn flush(&self) -> Result<()> {
        self.signal(Message::Flush).await
    }

    pub async fn stop(&self) -> Result<()> {
        self.signal(Message::Stop).await
    }

    async fn signal(&self, message: fn(oneshot::Sender<()>) -> Message) -> Result<()> {
        let (done, waited) = oneshot::channel();
        self.tx
            .send(message(done))
            .await
            .map_err(|_| KeelError::internal("audit.error.not_running"))?;
        waited
            .await
            .map_err(|_| KeelError::internal("audit.error.not_running"))
    }
}

impl AuditSink for Sink {
    fn record(&self, event: AuditEvent) {
        let function = event.function.clone();
        let request_id = event.request_id.clone();
        let refused = match self.tx.try_send(Message::Event(Box::new(event))) {
            Ok(()) => return,
            Err(TrySendError::Full(_)) => "full",
            Err(TrySendError::Closed(_)) => "closed",
        };
        let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            function,
            request_id,
            dropped,
            "the audit channel is {refused}; the event is dropped"
        );
    }
}

struct Batch {
    rows: Vec<call::ActiveModel>,
    waiters: Vec<oneshot::Sender<()>>,
    stop: Option<oneshot::Sender<()>>,
}

impl Batch {
    fn new() -> Self {
        Self {
            rows: Vec::with_capacity(BATCH_ROWS),
            waiters: Vec::new(),
            stop: None,
        }
    }

    fn take(&mut self, message: Message) {
        match message {
            Message::Event(event) => self.rows.push(row_of(*event)),
            Message::Flush(done) => self.waiters.push(done),
            Message::Stop(done) => self.stop = Some(done),
        }
    }

    fn is_open(&self) -> bool {
        self.stop.is_none() && self.rows.len() < BATCH_ROWS
    }
}

impl Worker {
    pub async fn run(mut self, db: Db) {
        while let Some(first) = self.rx.recv().await {
            let mut batch = Batch::new();
            batch.take(first);
            let deadline = Instant::now() + BATCH_WAIT;
            while batch.is_open() {
                match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                    Ok(Some(message)) => batch.take(message),
                    Ok(None) | Err(_) => break,
                }
            }
            if batch.stop.is_some() {
                while let Ok(message) = self.rx.try_recv() {
                    batch.take(message);
                }
            }
            write(&db, batch.rows).await;
            for done in batch.waiters {
                let _ = done.send(());
            }
            if let Some(done) = batch.stop {
                let _ = done.send(());
                return;
            }
        }
    }
}

async fn write(db: &Db, rows: Vec<call::ActiveModel>) {
    let mut rows = rows.into_iter().peekable();
    while rows.peek().is_some() {
        let chunk: Vec<call::ActiveModel> = rows.by_ref().take(BATCH_ROWS).collect();
        let count = chunk.len();
        if let Err(error) = call::Entity::insert_many(chunk).exec(db).await {
            tracing::error!(count, error = %error, "an audit batch could not be written");
        }
    }
}

pub fn row_of(event: AuditEvent) -> call::ActiveModel {
    let principal = event.principal;
    let (status, error_code, error_key) = match event.outcome {
        AuditOutcome::Committed => (STATUS_COMMITTED, None, None),
        AuditOutcome::Failed { error } => (
            STATUS_FAILED,
            Some(error.code.as_str().to_owned()),
            Some(error.key),
        ),
    };
    let actor = principal.actor.as_deref();
    call::ActiveModel {
        id: Set(Id::generate().to_string()),
        at: Set(event.at),
        request_id: Set(event.request_id),
        function: Set(event.function),
        module: Set(event.module),
        principal_id: Set((principal.level != Level::Guest).then(|| principal.id.to_string())),
        principal_login: Set(principal.login.clone()),
        account_id: Set(principal.account_id.to_string()),
        actor_principal_id: Set(actor.map(|actor| actor.id.to_string())),
        actor_login: Set(actor.map(|actor| actor.login.clone())),
        via: Set(via_text(&principal.via)),
        ip: Set(event
            .client
            .and_then(|client| client.ip)
            .map(|ip| ip.to_string())),
        params_redacted: Set(event.params),
        subject: Set(event.subject),
        after: Set(event.result),
        status: Set(status.to_owned()),
        error_code: Set(error_code),
        error_key: Set(error_key),
        elapsed_ms: Set(i64::try_from(event.elapsed.as_millis()).unwrap_or(i64::MAX)),
    }
}

pub fn via_text(via: &Via) -> String {
    match via {
        Via::Http => "http".to_owned(),
        Via::Socket => "socket".to_owned(),
        Via::Internal => "internal".to_owned(),
        Via::Su => "su".to_owned(),
        Via::Plugin(id) => format!("plugin:{id}"),
        Via::Peer(id) => format!("peer:{id}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use keel_sdk::sea_orm::ActiveValue;
    use keel_sdk::system::Client;
    use keel_sdk::types::{CallError, ErrorCode, Scope};

    use super::*;

    fn principal(level: Level, login: &str) -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().expect("an id"),
            login: login.to_owned(),
            level,
            account_id: "01J8A0000000000000000000A1".parse().expect("an id"),
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
        }
    }

    fn event(principal: Principal, outcome: AuditOutcome) -> AuditEvent {
        AuditEvent {
            request_id: "req-1".into(),
            function: "note.edit".into(),
            module: "note".into(),
            mode: Mode::Write,
            principal,
            client: Some(Client {
                ip: Some("10.0.0.7".parse().expect("an address")),
                user_agent: None,
                session: None,
            }),
            params: json!({ "id": "n1", "password": "***" }),
            subject: Some("n1".into()),
            result: Some(json!({ "id": "n1" })),
            outcome,
            at: "2026-09-13T20:00:00Z".parse().expect("an instant"),
            elapsed: Duration::from_millis(1234),
        }
    }

    fn set<T>(value: &ActiveValue<T>) -> T
    where
        T: Clone + Into<keel_sdk::sea_orm::Value>,
    {
        match value {
            ActiveValue::Set(value) | ActiveValue::Unchanged(value) => value.clone(),
            ActiveValue::NotSet => panic!("the column is not set"),
        }
    }

    #[test]
    fn a_row_carries_both_sides_of_the_call_and_a_guest_has_no_principal_id() {
        let mut delegated = principal(Level::User, "a1");
        let mut actor = principal(Level::Admin, "admin");
        actor.via = Via::Su;
        delegated.actor = Some(Box::new(actor));
        let row = row_of(event(delegated, AuditOutcome::Committed));
        assert_eq!(set(&row.principal_login), "a1");
        assert_eq!(
            set(&row.principal_id).as_deref(),
            Some("01J9Q3W6B1R8N5X2K4T7M0V9A1")
        );
        assert_eq!(set(&row.actor_login).as_deref(), Some("admin"));
        assert_eq!(
            set(&row.actor_principal_id).as_deref(),
            Some("01J9Q3W6B1R8N5X2K4T7M0V9A1")
        );
        assert_eq!(set(&row.via), "http");
        assert_eq!(set(&row.ip).as_deref(), Some("10.0.0.7"));
        assert_eq!(set(&row.status), STATUS_COMMITTED);
        assert_eq!(set(&row.error_code), None);
        assert_eq!(set(&row.subject).as_deref(), Some("n1"));
        assert_eq!(set(&row.after), Some(json!({ "id": "n1" })));
        assert_eq!(set(&row.elapsed_ms), 1234);

        let mut guest = principal(Level::Guest, "guest");
        guest.via = Via::Socket;
        let failed = AuditOutcome::Failed {
            error: CallError {
                key: "auth.error.credentials".into(),
                ..CallError::new(ErrorCode::Unauthenticated)
            },
        };
        let row = row_of(event(guest, failed));
        assert_eq!(set(&row.principal_id), None);
        assert_eq!(set(&row.actor_login), None);
        assert_eq!(set(&row.via), "socket");
        assert_eq!(set(&row.status), STATUS_FAILED);
        assert_eq!(set(&row.error_code).as_deref(), Some("unauthenticated"));
        assert_eq!(
            set(&row.error_key).as_deref(),
            Some("auth.error.credentials")
        );
    }

    #[test]
    fn via_is_written_as_a_word_and_plugins_and_peers_carry_their_id() {
        assert_eq!(via_text(&Via::Internal), "internal");
        assert_eq!(via_text(&Via::Su), "su");
        assert_eq!(via_text(&Via::Plugin("backup".into())), "plugin:backup");
        assert_eq!(
            via_text(&Via::Peer(
                "01J8A0000000000000000000A1".parse().expect("an id")
            )),
            "peer:01J8A0000000000000000000A1"
        );
    }

    #[tokio::test]
    async fn a_full_channel_drops_the_event_and_counts_it() {
        let (sink, worker) = channel();
        for _ in 0..CHANNEL_CAPACITY {
            sink.record(event(principal(Level::User, "a1"), AuditOutcome::Committed));
        }
        assert_eq!(sink.dropped(), 0);
        sink.record(event(principal(Level::User, "a1"), AuditOutcome::Committed));
        assert_eq!(sink.dropped(), 1);
        drop(worker);
        sink.record(event(principal(Level::User, "a1"), AuditOutcome::Committed));
        assert_eq!(sink.dropped(), 2);
        let error = sink.flush().await.expect_err("the worker is gone");
        assert_eq!(error.key(), "audit.error.not_running");
    }
}
