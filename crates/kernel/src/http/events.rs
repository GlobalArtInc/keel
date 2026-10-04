use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::extract::State;
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Uri};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use futures::stream;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Client, Credential, Frame, OutboxSubscriber};
use keel_sdk::{AccountScope, KeelError};
use keel_types::{Event, Id, Level, Principal};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::auth::{self, Caller};
use super::live::{Live, Resolvers};
use super::{Readiness, Shared, reply};
use crate::dispatcher::{OUTBOX_BATCH, OutboxFeed};
use crate::registry::Registry;

pub const SSE_REPLAY_WINDOW: u64 = 1000;
pub(super) const SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);
pub const SSE_RETRY: Duration = Duration::from_millis(3000);
pub const SSE_MAX_CLIENTS: usize = 4;
pub const LAST_EVENT_ID: HeaderName = HeaderName::from_static("last-event-id");
pub const DATA_INVALIDATE: &str = "data.invalidate";
pub const META_INVALIDATE: &str = "meta.invalidate";

const AFTER: &str = "after";
const NO_STORE: &str = "no-store";
pub(super) const KEEP_ALIVE_COMMENT: &str = "keep-alive";
const DRAINING_COMMENT: &str = "draining";
const SUPERSEDED_COMMENT: &str = "superseded";
pub(super) const KEEP_ALIVE_FLOOR: Duration = Duration::from_millis(1);
const QUEUE: usize = 64;

pub(super) type SessionKey = [u8; 32];

struct Slot {
    serial: u64,
    evict: oneshot::Sender<()>,
}

pub(super) struct Streams {
    readiness: Arc<Readiness>,
    open: AtomicUsize,
    serial: AtomicU64,
    sessions: Mutex<HashMap<SessionKey, VecDeque<Slot>>>,
}

impl Streams {
    pub(super) fn new(readiness: Arc<Readiness>) -> Self {
        Self {
            readiness,
            open: AtomicUsize::new(0),
            serial: AtomicU64::new(0),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    pub(super) fn enter(self: &Arc<Self>, key: SessionKey) -> (Opened, oneshot::Receiver<()>) {
        let serial = self.serial.fetch_add(1, Ordering::SeqCst);
        let (evict, evicted) = oneshot::channel();
        {
            let mut sessions = self.sessions();
            let slots = sessions.entry(key).or_default();
            while slots.len() >= SSE_MAX_CLIENTS
                && let Some(oldest) = slots.pop_front()
            {
                let _ = oldest.evict.send(());
            }
            slots.push_back(Slot { serial, evict });
        }
        self.open.fetch_add(1, Ordering::SeqCst);
        let opened = Opened {
            streams: self.clone(),
            key,
            serial,
        };
        (opened, evicted)
    }

    fn leave(&self, key: &SessionKey, serial: u64) {
        let mut sessions = self.sessions();
        if let Some(slots) = sessions.get_mut(key) {
            slots.retain(|slot| slot.serial != serial);
            if slots.is_empty() {
                sessions.remove(key);
            }
        }
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<SessionKey, VecDeque<Slot>>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn drained(&self) -> watch::Receiver<bool> {
        self.readiness.drained()
    }
}

pub(super) struct Opened {
    streams: Arc<Streams>,
    key: SessionKey,
    serial: u64,
}

impl Drop for Opened {
    fn drop(&mut self) {
        self.streams.leave(&self.key, self.serial);
        self.streams.open.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) fn session_key(credential: &Credential) -> SessionKey {
    let (kind, secret): (&[u8], _) = match credential {
        Credential::Bearer(secret) => (b"bearer", secret),
        Credential::Session(secret) => (b"session", secret),
    };
    Sha256::new()
        .chain_update(kind)
        .chain_update([0])
        .chain_update(secret.expose().as_bytes())
        .finalize()
        .into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    Fresh,
    After(u64),
    Unreadable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Start {
    cursor: u64,
    stale: bool,
}

pub(super) async fn serve(
    State(state): State<Arc<Shared>>,
    live: Live,
    caller: Caller,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let request_id = caller.request_id;
    let credential = match auth::credential(&headers) {
        Some(credential) if caller.principal.level != Level::Guest => credential,
        _ => {
            return reply::error(
                KeelError::unauthenticated("keel.error.unauthenticated"),
                &request_id,
            );
        }
    };
    let (Some(bus), Some(feed)) = (live.bus.clone(), live.outbox.clone()) else {
        return reply::error(
            KeelError::unavailable("keel.error.unavailable"),
            &request_id,
        );
    };
    let frames = bus.subscribe();
    let latest = feed.latest();
    let newest = match feed.newest().await {
        Ok(newest) => newest,
        Err(error) => return reply::error(error, &request_id),
    };
    let local = match feed.instance() {
        Ok(local) => local,
        Err(error) => return reply::error(error, &request_id),
    };
    let resume = resume_point(&headers, uri.query());
    let start = start(resume, newest, state.options.sse_replay_window);
    let (opened, evicted) = state.streams.enter(session_key(&credential));
    let drained = state.streams.drained();
    let (tx, rx) = mpsc::channel(QUEUE);
    tracing::debug!(
        request_id,
        principal = %caller.principal.id,
        login = caller.principal.login,
        cursor = start.cursor,
        stale = start.stale,
        open = state.streams.open(),
        "event stream opened"
    );
    let connection = Connection {
        request_id,
        scope: AccountScope::of(&caller.principal),
        principal: caller.principal,
        credential,
        on_behalf: auth::on_behalf(&headers),
        client: caller.client,
        resolvers: live.resolvers.clone(),
        keep_alive: state.options.sse_keep_alive.max(KEEP_ALIVE_FLOOR),
        db: state.db.clone(),
        registry: live.registry.clone(),
        feed,
        local,
        frames,
        latest,
        cursor: start.cursor,
        stale: start.stale,
        tx,
    };
    tokio::spawn(connection.run(drained, evicted, opened));
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

fn resume_point(headers: &HeaderMap, query: Option<&str>) -> Resume {
    let header = headers
        .get(LAST_EVENT_ID)
        .map(|value| value.to_str().ok().map(str::trim).map(str::to_owned));
    let member = query.and_then(|query| {
        query
            .split('&')
            .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
            .find(|(name, _)| *name == AFTER)
            .map(|(_, value)| Some(value.trim().to_owned()))
    });
    match header.or(member) {
        None => Resume::Fresh,
        Some(given) => given
            .and_then(|text| text.parse::<u64>().ok())
            .map_or(Resume::Unreadable, Resume::After),
    }
}

fn start(resume: Resume, newest: u64, window: u64) -> Start {
    match resume {
        Resume::Fresh => Start {
            cursor: newest,
            stale: false,
        },
        Resume::After(id) if id <= newest && newest - id <= window => Start {
            cursor: id,
            stale: false,
        },
        Resume::After(_) | Resume::Unreadable => Start {
            cursor: newest,
            stale: true,
        },
    }
}

struct Connection {
    request_id: String,
    principal: Principal,
    scope: AccountScope,
    credential: Credential,
    on_behalf: Option<String>,
    client: Client,
    resolvers: Resolvers,
    keep_alive: Duration,
    db: DatabaseConnection,
    registry: Arc<Registry>,
    feed: Arc<OutboxFeed>,
    local: Id,
    frames: broadcast::Receiver<Frame>,
    latest: watch::Receiver<u64>,
    cursor: u64,
    stale: bool,
    tx: mpsc::Sender<SseEvent>,
}

impl Connection {
    async fn run(
        mut self,
        mut drained: watch::Receiver<bool>,
        mut evicted: oneshot::Receiver<()>,
        opened: Opened,
    ) {
        match self.pump(&mut drained, &mut evicted).await {
            Ok(()) => tracing::debug!(
                request_id = self.request_id,
                principal = %self.principal.id,
                "event stream closed"
            ),
            Err(error) => tracing::warn!(
                request_id = self.request_id,
                principal = %self.principal.id,
                error = %error,
                "event stream closed on an error"
            ),
        }
        drop(opened);
    }

    async fn pump(
        &mut self,
        drained: &mut watch::Receiver<bool>,
        evicted: &mut oneshot::Receiver<()>,
    ) -> Result<(), KeelError> {
        if !self.send(SseEvent::default().retry(SSE_RETRY)).await {
            return Ok(());
        }
        if self.stale && !self.meta_invalidate().await {
            return Ok(());
        }
        if !self.catch_up().await? {
            return Ok(());
        }
        let mut keep_alive =
            tokio::time::interval_at(Instant::now() + self.keep_alive, self.keep_alive);
        keep_alive.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.tx.closed() => return Ok(()),
                () = draining(drained) => {
                    self.send(SseEvent::default().retry(SSE_RETRY).comment(DRAINING_COMMENT)).await;
                    return Ok(());
                }
                superseded = &mut *evicted => {
                    if superseded.is_ok() {
                        self.send(SseEvent::default().retry(Duration::ZERO).comment(SUPERSEDED_COMMENT)).await;
                    }
                    return Ok(());
                }
                changed = self.latest.changed() => {
                    if changed.is_err() || !self.catch_up().await? {
                        return Ok(());
                    }
                }
                frame = self.frames.recv() => match frame {
                    Ok(frame) => {
                        if !self.forward(frame).await? {
                            return Ok(());
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => tracing::debug!(
                        request_id = self.request_id,
                        skipped,
                        "event stream fell behind the live frames"
                    ),
                    Err(RecvError::Closed) => return Ok(()),
                },
                _ = keep_alive.tick() => {
                    if !self.recheck().await
                        || !self.send(SseEvent::default().comment(KEEP_ALIVE_COMMENT)).await
                    {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn recheck(&mut self) -> bool {
        let db = Db::from_connection(self.db.clone());
        match auth::resolve(
            &self.resolvers,
            &self.credential,
            self.on_behalf.as_deref(),
            &self.client,
            &db,
        )
        .await
        {
            Ok(authenticated) => {
                self.scope = AccountScope::of(&authenticated.principal);
                self.principal = authenticated.principal;
                true
            }
            Err(error) if auth::refuses(&error) => {
                tracing::debug!(
                    request_id = self.request_id,
                    principal = %self.principal.id,
                    key = error.key(),
                    "the event stream's credential no longer admits it"
                );
                false
            }
            Err(error) => {
                tracing::warn!(
                    request_id = self.request_id,
                    principal = %self.principal.id,
                    error = %error,
                    "the event stream's credential could not be checked again"
                );
                true
            }
        }
    }

    async fn catch_up(&mut self) -> Result<bool, KeelError> {
        loop {
            let events = self.feed.after(self.cursor, OUTBOX_BATCH).await?;
            let count = events.len();
            for event in events {
                self.cursor = self.cursor.max(event.seq);
                if admits_event(&self.principal, &self.scope, &event, self.local, &self.db).await?
                    && !self.send(invalidate(&event)?).await
                {
                    return Ok(false);
                }
            }
            if count < OUTBOX_BATCH as usize {
                return Ok(true);
            }
        }
    }

    async fn forward(&mut self, frame: Frame) -> Result<bool, KeelError> {
        if frame.kind.contains(['\n', '\r'])
            || !admits_frame(&self.principal, &self.scope, &frame, &self.db).await?
        {
            return Ok(true);
        }
        let event = SseEvent::default()
            .event(&frame.kind)
            .data(serde_json::to_string(&frame.data)?);
        Ok(self.send(event).await)
    }

    async fn meta_invalidate(&mut self) -> bool {
        let meta = match crate::meta::version(&self.registry, &self.db, &self.principal).await {
            Ok(version) => Value::String(version),
            Err(error) => {
                tracing::warn!(
                    request_id = self.request_id,
                    error = %error,
                    "the metadata version could not be read for the event stream"
                );
                Value::Null
            }
        };
        self.send(
            SseEvent::default()
                .event(META_INVALIDATE)
                .data(json!({ "meta": meta }).to_string()),
        )
        .await
    }

    async fn send(&self, event: SseEvent) -> bool {
        self.tx.send(event).await.is_ok()
    }
}

pub(super) async fn draining(drained: &mut watch::Receiver<bool>) {
    if drained.wait_for(|draining| *draining).await.is_err() {
        std::future::pending::<()>().await;
    }
}

fn invalidate(event: &Event) -> Result<SseEvent, KeelError> {
    Ok(SseEvent::default()
        .event(DATA_INVALIDATE)
        .id(event.seq.to_string())
        .data(serde_json::to_string(event)?))
}

async fn admits_event(
    principal: &Principal,
    scope: &AccountScope,
    event: &Event,
    local: Id,
    db: &DatabaseConnection,
) -> Result<bool, KeelError> {
    if event.instance_id != local {
        return Ok(*scope == AccountScope::All && !principal.is_narrowed());
    }
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

async fn admits_frame(
    principal: &Principal,
    scope: &AccountScope,
    frame: &Frame,
    db: &DatabaseConnection,
) -> Result<bool, KeelError> {
    if frame.principal == principal.id {
        return Ok(true);
    }
    Ok(match scope {
        AccountScope::All => true,
        AccountScope::Subtree(_) => scope.includes(db, frame.account).await?,
        AccountScope::Own(_) | AccountScope::Nothing => false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::Utc;
    use keel_sdk::Secret;
    use keel_types::{EventSubject, PeerScope, Scope, Via};

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(*name, HeaderValue::from_static(value));
        }
        headers
    }

    fn principal(level: Level, account: Id) -> Principal {
        Principal {
            id: Id::generate(),
            login: "p".into(),
            level,
            account_id: account,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: BTreeMap::new(),
        }
    }

    fn local() -> Id {
        "01J9K00000000000000000000K".parse().expect("an id")
    }

    fn event(account: Option<Id>) -> Event {
        emitted_by(local(), account)
    }

    fn emitted_by(instance: Id, account: Option<Id>) -> Event {
        Event {
            id: Id::generate(),
            seq: 7,
            kind: "shelf.book.saved".into(),
            at: Utc::now(),
            instance_id: instance,
            module: "shelf".into(),
            actor: None,
            subject: Some(EventSubject {
                kind: "shelf.book".into(),
                id: Id::generate(),
                account_id: account,
            }),
            data: json!({}),
        }
    }

    fn frame(principal: Id, account: Id) -> Frame {
        Frame {
            principal,
            account,
            kind: "task.progress".into(),
            data: json!({ "done": 1 }),
        }
    }

    #[test]
    fn the_resume_point_comes_from_the_header_before_the_query_member() {
        assert_eq!(resume_point(&headers(&[]), None), Resume::Fresh);
        assert_eq!(
            resume_point(&headers(&[("last-event-id", " 12 ")]), None),
            Resume::After(12)
        );
        assert_eq!(
            resume_point(&headers(&[]), Some("after=5&lang=en")),
            Resume::After(5)
        );
        assert_eq!(
            resume_point(&headers(&[("last-event-id", "9")]), Some("after=5")),
            Resume::After(9)
        );
        assert_eq!(
            resume_point(&headers(&[("last-event-id", "abc")]), Some("after=5")),
            Resume::Unreadable
        );
        assert_eq!(
            resume_point(&headers(&[]), Some("after=-1")),
            Resume::Unreadable
        );
        assert_eq!(
            resume_point(&headers(&[]), Some("after")),
            Resume::Unreadable
        );
        assert_eq!(resume_point(&headers(&[]), Some("lang=en")), Resume::Fresh);
    }

    #[test]
    fn an_id_inside_the_window_replays_and_one_outside_or_unreadable_marks_the_stream_stale() {
        let fresh = start(Resume::Fresh, 40, 10);
        assert_eq!(
            fresh,
            Start {
                cursor: 40,
                stale: false
            }
        );
        assert_eq!(
            start(Resume::After(30), 40, 10),
            Start {
                cursor: 30,
                stale: false
            }
        );
        assert_eq!(
            start(Resume::After(40), 40, 10),
            Start {
                cursor: 40,
                stale: false
            }
        );
        assert_eq!(
            start(Resume::After(29), 40, 10),
            Start {
                cursor: 40,
                stale: true
            }
        );
        assert_eq!(
            start(Resume::After(41), 40, 10),
            Start {
                cursor: 40,
                stale: true
            }
        );
        assert_eq!(
            start(Resume::Unreadable, 40, 10),
            Start {
                cursor: 40,
                stale: true
            }
        );
    }

    async fn admitted(principal: &Principal, scope: AccountScope, event: &Event) -> bool {
        admits_event(
            principal,
            &scope,
            event,
            local(),
            &DatabaseConnection::Disconnected,
        )
        .await
        .expect("no account lookup is needed")
    }

    #[tokio::test]
    async fn an_event_reaches_the_scopes_that_hold_its_account_and_one_without_reaches_all_alone() {
        let a1 = Id::generate();
        let a2 = Id::generate();
        let who = principal(Level::Admin, Id::ROOT_ACCOUNT);
        assert!(admitted(&who, AccountScope::All, &event(Some(a1))).await);
        assert!(admitted(&who, AccountScope::All, &event(None)).await);
        assert!(admitted(&who, AccountScope::Own(a1), &event(Some(a1))).await);
        assert!(!admitted(&who, AccountScope::Own(a1), &event(Some(a2))).await);
        assert!(!admitted(&who, AccountScope::Own(a1), &event(None)).await);
        assert!(!admitted(&who, AccountScope::Subtree(a1), &event(None)).await);
        assert!(!admitted(&who, AccountScope::Nothing, &event(Some(a1))).await);
    }

    #[tokio::test]
    async fn an_event_of_another_installation_reaches_an_admin_alone_whose_credential_is_not_narrowed()
     {
        let other = Id::generate();
        let root = Some(Id::ROOT_ACCOUNT);
        let admin = principal(Level::Admin, Id::ROOT_ACCOUNT);
        let relayed = emitted_by(other, root);
        assert!(admitted(&admin, AccountScope::All, &relayed).await);
        assert!(admitted(&admin, AccountScope::All, &emitted_by(other, None)).await);
        let mut read_only = admin.clone();
        read_only.scope = Scope::ReadOnly;
        assert!(admitted(&read_only, AccountScope::All, &relayed).await);

        let service = principal(Level::Service, Id::ROOT_ACCOUNT);
        let user = principal(Level::User, Id::ROOT_ACCOUNT);
        for below in [&service, &user] {
            assert!(admitted(below, AccountScope::of(below), &event(root)).await);
            assert!(!admitted(below, AccountScope::of(below), &relayed).await);
        }
        let reseller = principal(Level::Reseller, Id::ROOT_ACCOUNT);
        assert!(!admitted(&reseller, AccountScope::Subtree(Id::ROOT_ACCOUNT), &relayed).await);

        let mut token = admin.clone();
        token.scope = Scope::Functions(vec!["shelf.*".into()]);
        assert!(admitted(&token, AccountScope::All, &event(root)).await);
        assert!(!admitted(&token, AccountScope::All, &relayed).await);
        let mut link = admin.clone();
        link.scope = Scope::Peer(PeerScope {
            functions: Vec::new(),
            level: Level::Admin,
            on_behalf: false,
        });
        link.via = Via::Peer(Id::generate());
        assert!(admitted(&link, AccountScope::of(&link), &event(root)).await);
        assert!(!admitted(&link, AccountScope::of(&link), &relayed).await);
    }

    #[tokio::test]
    async fn a_frame_reaches_its_principal_and_every_admin_and_no_other_user() {
        let db = DatabaseConnection::Disconnected;
        let a1 = Id::generate();
        let user = principal(Level::User, a1);
        let other = principal(Level::User, a1);
        let admin = principal(Level::Admin, Id::ROOT_ACCOUNT);
        let own = frame(user.id, a1);
        assert!(
            admits_frame(&user, &AccountScope::of(&user), &own, &db)
                .await
                .unwrap()
        );
        assert!(
            !admits_frame(&other, &AccountScope::of(&other), &own, &db)
                .await
                .unwrap()
        );
        assert!(
            admits_frame(&admin, &AccountScope::of(&admin), &own, &db)
                .await
                .unwrap()
        );
        let guest = principal(Level::Guest, Id::ROOT_ACCOUNT);
        assert!(
            !admits_frame(&guest, &AccountScope::of(&guest), &own, &db)
                .await
                .unwrap()
        );
    }

    #[test]
    fn the_invalidate_frame_carries_the_seq_as_its_id_and_the_event_as_its_data() {
        let event = event(None);
        let frame = invalidate(&event).expect("the event serialises");
        let text = format!("{frame:?}");
        assert!(text.contains("id: 7"), "{text}");
        assert!(text.contains("event: data.invalidate"), "{text}");
        assert!(text.contains("\\\"seq\\\":7"), "{text}");
    }

    fn evicted(receiver: &mut oneshot::Receiver<()>) -> bool {
        match receiver.try_recv() {
            Ok(()) => true,
            Err(oneshot::error::TryRecvError::Empty) => false,
            Err(error) => panic!("the eviction channel closed: {error}"),
        }
    }

    #[test]
    fn a_session_beyond_the_cap_evicts_its_oldest_stream_and_a_closed_stream_frees_its_slot() {
        let streams = Arc::new(Streams::new(Readiness::new()));
        let session = session_key(&Credential::Session(Secret::new("ks_a")));
        let token = session_key(&Credential::Bearer(Secret::new("ks_a")));
        assert_ne!(session, token);
        let mut held: Vec<_> = (0..SSE_MAX_CLIENTS)
            .map(|_| streams.enter(session))
            .collect();
        let (_other, mut other_evicted) = streams.enter(token);
        drop(held.remove(1));
        held.push(streams.enter(session));
        assert!(held.iter_mut().all(|(_, receiver)| !evicted(receiver)));
        assert_eq!(streams.open(), SSE_MAX_CLIENTS + 1);

        held.push(streams.enter(session));
        assert!(evicted(&mut held[0].1), "the oldest stream is evicted");
        assert!(held[1..].iter_mut().all(|(_, receiver)| !evicted(receiver)));
        assert!(
            !evicted(&mut other_evicted),
            "another credential keeps its stream"
        );
        drop(held);
        assert_eq!(streams.open(), 1);
        assert!(streams.sessions().get(&session).is_none());
    }
}
