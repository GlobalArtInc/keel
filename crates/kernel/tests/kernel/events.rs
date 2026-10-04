use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use futures::{StreamExt, stream};
use keel_kernel::dispatcher::OUTBOX_BATCH;
use keel_kernel::http::{
    DATA_INVALIDATE, END_EVENT, LAST_EVENT_ID, LINE_EVENT, META_INVALIDATE, ON_BEHALF_OF,
    SESSION_COOKIE, SSE_MAX_CLIENTS, SSE_RETRY,
};
use keel_kernel::{Http, HttpOptions};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{
    Authenticated, Authenticator, Client, Credential, EventRelay, Frame, LiveEvents, OnBehalf,
    OutboxSubscriber,
};
use keel_testkit::http::with_test_principal;
use keel_testkit::{EVENTS_PATH, EventStream, Harness, PRINCIPAL_HEADER, SseFrame, TestHttp, Who};
use keel_types::{Event, PeerScope, Scope, Via};
use tokio::sync::broadcast;

const QUIET: Duration = Duration::from_millis(700);
const PATIENCE: Duration = Duration::from_secs(5);
const STOP_WITHIN: Duration = Duration::from_secs(1);
const KEEP_ALIVE: Duration = Duration::from_millis(200);
const TAIL_FUNCTION: &str = "stream.tail.follow";
const TAIL_PATH: &str = "/api/v1/x/stream/tail.follow";

static CREDENTIALS: LazyLock<Mutex<HashMap<String, Principal>>> = LazyLock::new(Mutex::default);
static DELEGATIONS: LazyLock<Mutex<HashMap<String, Principal>>> = LazyLock::new(Mutex::default);
static REFUSALS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
static TAIL: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(16).0);

#[derive(Deserialize, Serialize, JsonSchema)]
struct Labelled {
    label: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

static STREAM: ModuleDef = ModuleDef {
    id: "stream",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Labelled, Nothing>("stream.item.touch")
            .kind(Kind::Action)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Labelled>| async move {
                call.emit_about(
                    "stream.item.touched",
                    EventSubject {
                        kind: "stream.item".into(),
                        id: Id::generate(),
                        account_id: Some(call.principal().account_id),
                    },
                    &json!({ "label": call.params().label }),
                )?;
                Ok(Nothing {})
            }),
    )
    .function(
        function::<Labelled, Nothing>("stream.notice.post")
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|call: Call<Labelled>| async move {
                call.emit_about(
                    "stream.notice.posted",
                    EventSubject {
                        kind: "stream.notice".into(),
                        id: Id::generate(),
                        account_id: None,
                    },
                    &json!({ "label": call.params().label }),
                )?;
                Ok(Nothing {})
            }),
    )
    .stream(
        function::<Nothing, LogLine>(TAIL_FUNCTION)
            .level(Level::User)
            .unowned(),
        follow,
    );
    Ok(())
}

async fn follow(call: Call<Nothing>) -> Result<Lines> {
    let first = LogLine::new(LogLevel::Info, call.principal().login.clone());
    let live = stream::unfold(TAIL.subscribe(), |mut tail| async move {
        tail.recv()
            .await
            .ok()
            .map(|text| (LogLine::new(LogLevel::Info, text), tail))
    });
    Ok(Lines::new(stream::once(async { first }).chain(live)))
}

fn refused(key: &str) -> bool {
    REFUSALS.lock().expect("the refusals").contains(key)
}

fn refuse(key: &str) {
    REFUSALS
        .lock()
        .expect("the refusals")
        .insert(key.to_owned());
}

static CREDENTIAL: ModuleDef = ModuleDef {
    id: "credential",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: init_credential,
};

struct Revocable;

#[async_trait]
impl Authenticator for Revocable {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        _: &Db,
    ) -> Result<Option<Authenticated>> {
        let (Credential::Bearer(secret) | Credential::Session(secret)) = credential;
        if refused(secret.expose()) {
            return Err(KeelError::forbidden("credential.error.refused"));
        }
        let principal = CREDENTIALS
            .lock()
            .expect("the credentials")
            .get(secret.expose())
            .cloned();
        Ok(principal.map(|principal| Authenticated {
            principal,
            session: None,
        }))
    }
}

struct Delegating;

#[async_trait]
impl OnBehalf for Delegating {
    async fn act_for(&self, caller: &Principal, header: &str, _: &Db) -> Result<Option<Principal>> {
        if caller.level < Level::Admin {
            return Ok(None);
        }
        if refused(header) {
            return Err(KeelError::forbidden("stream.error.refused"));
        }
        let Some(mut principal) = DELEGATIONS
            .lock()
            .expect("the delegations")
            .get(header)
            .cloned()
        else {
            return Err(KeelError::not_found("stream.error.customer"));
        };
        let mut actor = caller.clone();
        actor.via = Via::Peer(Id::ROOT_ACCOUNT);
        principal.via = Via::Peer(Id::ROOT_ACCOUNT);
        principal.actor = Some(Box::new(actor));
        Ok(Some(principal))
    }
}

fn init_credential(r: &mut Registrar) -> Result<()> {
    let authenticator: Arc<dyn Authenticator> = Arc::new(Revocable);
    let delegate: Arc<dyn OnBehalf> = Arc::new(Delegating);
    r.contribute(authenticator).contribute(delegate);
    Ok(())
}

fn issue(harness: &Harness, who: Who, prefix: &str) -> String {
    let secret = format!("{prefix}{}", Id::generate());
    CREDENTIALS
        .lock()
        .expect("the credentials")
        .insert(secret.clone(), harness.principal(who).clone());
    secret
}

fn revoke(secret: &str) {
    CREDENTIALS.lock().expect("the credentials").remove(secret);
}

fn cookie(secret: &str) -> String {
    format!("{SESSION_COOKIE}={secret}")
}

async fn boot() -> Harness {
    Harness::new().module(&STREAM).boot().await
}

async fn boot_with_credentials() -> Harness {
    Harness::new()
        .module(&STREAM)
        .module(&CREDENTIAL)
        .boot()
        .await
}

async fn opened_with(http: &TestHttp, headers: &[(&str, &str)]) -> EventStream {
    let mut stream = http.stream(EVENTS_PATH, headers).await;
    assert_eq!(stream.status(), 200, "{headers:?}");
    let first = stream.next().await;
    assert_eq!(first.retry, Some(3000), "{first:?}");
    stream
}

fn subscriber(harness: &Harness) -> Arc<dyn OutboxSubscriber> {
    harness
        .registry()
        .services()
        .get::<dyn OutboxSubscriber>()
        .expect("the kernel provides the outbox subscriber")
}

fn live(harness: &Harness) -> Arc<dyn LiveEvents> {
    harness
        .registry()
        .services()
        .get::<dyn LiveEvents>()
        .expect("the kernel provides the live events")
}

async fn touch(harness: &Harness, who: Who, label: &str) -> u64 {
    let reply = harness
        .call(who, "stream.item.touch", json!({ "label": label }))
        .await;
    assert_eq!(reply.status(), 200, "{who}: {:?}", reply.error());
    *subscriber(harness).latest().borrow()
}

async fn post(harness: &Harness, label: &str) -> u64 {
    let reply = harness
        .call(Who::Admin, "stream.notice.post", json!({ "label": label }))
        .await;
    assert_eq!(reply.status(), 200, "{:?}", reply.error());
    *subscriber(harness).latest().borrow()
}

async fn outbox(harness: &Harness) -> Vec<Event> {
    subscriber(harness)
        .after(0, OUTBOX_BATCH)
        .await
        .expect("the outbox answers")
}

async fn opened(http: &TestHttp, who: Who, last_event_id: Option<u64>) -> EventStream {
    let mut stream = http.events(who, last_event_id).await;
    assert_eq!(stream.status(), 200, "{who}");
    assert_eq!(stream.header("content-type"), Some("text/event-stream"));
    assert_eq!(stream.header("cache-control"), Some("no-store"));
    let first = stream.next().await;
    assert_eq!(
        first.retry,
        Some(u64::try_from(SSE_RETRY.as_millis()).expect("millis")),
        "{who}: {first:?}"
    );
    assert!(first.is_comment(), "{who}: {first:?}");
    stream
}

fn invalidate(frame: &SseFrame) -> (u64, String, Option<Id>) {
    assert_eq!(frame.event.as_deref(), Some(DATA_INVALIDATE), "{frame:?}");
    let event: Event = serde_json::from_value(frame.json()).expect("an event");
    let id: u64 = frame
        .id
        .as_deref()
        .expect("an id")
        .parse()
        .expect("the id is the seq");
    assert_eq!(id, event.seq, "{frame:?}");
    (
        event.seq,
        event.kind,
        event.subject.and_then(|subject| subject.account_id),
    )
}

fn frame(principal: Id, account: Id, kind: &str, label: &str) -> Frame {
    Frame {
        principal,
        account,
        kind: kind.to_owned(),
        data: json!({ "label": label }),
    }
}

async fn live_event(stream: &mut EventStream, kind: &str) -> Value {
    let frame = stream.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(kind), "{frame:?}");
    assert_eq!(frame.id, None, "{frame:?}");
    frame.json()
}

#[tokio::test]
async fn a_user_receives_data_invalidate_for_its_own_objects_alone_while_an_admin_receives_every_one()
 {
    let harness = boot().await;
    let http = harness.http().await;
    let mut a1 = opened(&http, Who::A1, None).await;
    let mut a2 = opened(&http, Who::A2, None).await;
    let mut admin = opened(&http, Who::Admin, None).await;
    let first = touch(&harness, Who::A1, "first").await;
    let second = touch(&harness, Who::A2, "second").await;
    assert!(first < second);
    let events = outbox(&harness).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.module == "stream")
            .count(),
        2
    );
    let a1_account = harness.fixtures().accounts.a1;
    let a2_account = harness.fixtures().accounts.a2;

    let frame = a1.next_event().await;
    assert_eq!(
        invalidate(&frame),
        (first, "stream.item.touched".to_owned(), Some(a1_account))
    );
    assert_eq!(frame.json()["data"]["label"], json!("first"));
    assert_eq!(frame.json()["type"], json!("stream.item.touched"));
    assert_eq!(
        frame.json()["instanceId"],
        json!(harness.instance().to_string())
    );
    assert!(a1.quiet(QUIET).await, "a1 sees only its own object");

    let frame = a2.next_event().await;
    assert_eq!(
        invalidate(&frame),
        (second, "stream.item.touched".to_owned(), Some(a2_account))
    );
    assert!(a2.quiet(QUIET).await);

    let frame = admin.next_event().await;
    assert_eq!(invalidate(&frame).0, first);
    let frame = admin.next_event().await;
    assert_eq!(invalidate(&frame).0, second);

    let notice = post(&harness, "everyone").await;
    let frame = admin.next_event().await;
    assert_eq!(
        invalidate(&frame),
        (notice, "stream.notice.posted".to_owned(), None)
    );
    assert!(
        a1.quiet(QUIET).await,
        "an event without an account is for admins alone"
    );
    assert!(a2.quiet(QUIET).await);
    assert!(admin.quiet(QUIET).await);
    drop((a1, a2, admin));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

fn issued_as(principal: Principal) -> String {
    let secret = format!("narrowed-{}", Id::generate());
    CREDENTIALS
        .lock()
        .expect("the credentials")
        .insert(secret.clone(), principal);
    secret
}

#[tokio::test]
async fn an_event_of_another_installation_reaches_an_admin_alone_and_never_a_narrowed_credential() {
    let harness = boot_with_credentials().await;
    let http = harness.http().await;
    let admin = harness.principal(Who::Admin).clone();
    let mut scoped = admin.clone();
    scoped.scope = Scope::Functions(vec!["stream.*".into()]);
    let mut link = admin.clone();
    link.scope = Scope::Peer(PeerScope {
        functions: Vec::new(),
        level: Level::Admin,
        on_behalf: false,
    });
    link.via = Via::Peer(Id::generate());
    let scoped = format!("Bearer {}", issued_as(scoped));
    let link = format!("Bearer {}", issued_as(link));
    assert_eq!(harness.principal(Who::Service).account_id, Id::ROOT_ACCOUNT);

    let mut admin = opened(&http, Who::Admin, None).await;
    let mut service = opened(&http, Who::Service, None).await;
    let mut scoped = opened_with(&http, &[("authorization", scoped.as_str())]).await;
    let mut link = opened_with(&http, &[("authorization", link.as_str())]).await;

    let elsewhere = Id::generate();
    let relayed = Event {
        id: Id::generate(),
        seq: 1,
        kind: "stream.item.touched".into(),
        at: keel_sdk::chrono::Utc::now(),
        instance_id: elsewhere,
        module: "stream".into(),
        actor: None,
        subject: Some(EventSubject {
            kind: "stream.item".into(),
            id: Id::generate(),
            account_id: Some(Id::ROOT_ACCOUNT),
        }),
        data: json!({ "label": "elsewhere" }),
    };
    let relayed_seq = harness
        .registry()
        .services()
        .get::<dyn EventRelay>()
        .expect("the kernel relays events of peers")
        .relay(&relayed)
        .await
        .expect("the relay answers")
        .expect("the event enters the outbox");
    let here = touch(&harness, Who::Admin, "here").await;
    assert!(relayed_seq < here);

    let frame = admin.next_event().await;
    assert_eq!(
        invalidate(&frame),
        (
            relayed_seq,
            "stream.item.touched".to_owned(),
            Some(Id::ROOT_ACCOUNT)
        )
    );
    assert_eq!(frame.json()["instanceId"], json!(elsewhere.to_string()));
    let frame = admin.next_event().await;
    assert_eq!(invalidate(&frame).0, here);

    for (name, stream) in [
        ("a service principal of the root account", &mut service),
        ("an admin token scoped to functions", &mut scoped),
        ("an admin calling through a peer", &mut link),
    ] {
        let frame = stream.next_event().await;
        assert_eq!(
            invalidate(&frame),
            (
                here,
                "stream.item.touched".to_owned(),
                Some(Id::ROOT_ACCOUNT)
            ),
            "{name} receives the event of this installation and not the relayed one"
        );
        assert_eq!(
            frame.json()["instanceId"],
            json!(harness.instance().to_string()),
            "{name}"
        );
        assert!(stream.quiet(QUIET).await, "{name}");
    }
    assert!(admin.quiet(QUIET).await);
    drop((admin, service, scoped, link));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_reconnect_replays_after_last_event_id_and_an_id_outside_the_window_gets_meta_invalidate()
{
    let harness = boot().await;
    let first = touch(&harness, Who::A1, "one").await;
    let second = touch(&harness, Who::A1, "two").await;
    let foreign = touch(&harness, Who::A2, "foreign").await;
    let third = touch(&harness, Who::A1, "three").await;
    assert!(first < second && second < foreign && foreign < third);

    let http = harness.http().await;
    let mut resumed = opened(&http, Who::A1, Some(first)).await;
    let frame = resumed.next_event().await;
    assert_eq!(invalidate(&frame).0, second);
    let frame = resumed.next_event().await;
    assert_eq!(invalidate(&frame).0, third);
    assert!(
        resumed.quiet(QUIET).await,
        "the foreign event is filtered out of the replay"
    );
    let fourth = touch(&harness, Who::A1, "four").await;
    let frame = resumed.next_event().await;
    assert_eq!(invalidate(&frame).0, fourth);

    let mut current = opened(&http, Who::A1, Some(fourth)).await;
    assert!(
        current.quiet(QUIET).await,
        "nothing to replay at the latest seq"
    );

    let mut fresh = opened(&http, Who::A1, None).await;
    assert!(fresh.quiet(QUIET).await, "a fresh stream starts live");
    drop((resumed, current, fresh));
    http.stop().await.expect("the server stops");

    let narrow = harness
        .http_with(HttpOptions {
            sse_replay_window: 2,
            ..HttpOptions::default()
        })
        .await;
    let mut inside = opened(&narrow, Who::A1, Some(third)).await;
    let frame = inside.next_event().await;
    assert_eq!(invalidate(&frame).0, fourth);
    assert!(inside.quiet(QUIET).await);

    let mut outside = opened(&narrow, Who::A1, Some(first)).await;
    let frame = outside.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(META_INVALIDATE), "{frame:?}");
    assert_eq!(frame.id, None);
    let meta = frame.json();
    assert!(
        meta["meta"]
            .as_str()
            .is_some_and(|version| version.starts_with("m-")),
        "{meta}"
    );
    assert!(
        outside.quiet(QUIET).await,
        "nothing old is replayed outside the window"
    );
    let fifth = touch(&harness, Who::A1, "five").await;
    let frame = outside.next_event().await;
    assert_eq!(invalidate(&frame).0, fifth);

    let header = LAST_EVENT_ID;
    let mut unreadable = narrow
        .stream(
            EVENTS_PATH,
            &[
                (PRINCIPAL_HEADER, Who::A1.login()),
                (header.as_str(), "not-a-seq"),
            ],
        )
        .await;
    assert_eq!(unreadable.status(), 200);
    assert!(unreadable.next().await.is_comment());
    let frame = unreadable.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(META_INVALIDATE), "{frame:?}");

    let mut ahead = opened(&narrow, Who::A1, Some(fifth + 1000)).await;
    let frame = ahead.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(META_INVALIDATE), "{frame:?}");
    drop((inside, outside, unreadable, ahead));
    narrow.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn the_after_query_member_is_read_like_the_header() {
    let harness = boot().await;
    let first = touch(&harness, Who::A1, "one").await;
    let second = touch(&harness, Who::A1, "two").await;
    let http = harness.http().await;
    let mut queried = http
        .stream(
            &format!("{EVENTS_PATH}?after={first}"),
            &[(PRINCIPAL_HEADER, Who::A1.login())],
        )
        .await;
    assert_eq!(queried.status(), 200);
    assert!(queried.next().await.is_comment());
    let frame = queried.next_event().await;
    assert_eq!(invalidate(&frame).0, second);
    assert!(queried.quiet(QUIET).await);

    let header = LAST_EVENT_ID;
    let mut both = http
        .stream(
            &format!("{EVENTS_PATH}?after=0"),
            &[
                (PRINCIPAL_HEADER, Who::A1.login()),
                (header.as_str(), &first.to_string()),
            ],
        )
        .await;
    assert!(both.next().await.is_comment());
    let frame = both.next_event().await;
    assert_eq!(
        invalidate(&frame).0,
        second,
        "the header wins over the query member"
    );
    assert!(both.quiet(QUIET).await);

    let mut garbage = http
        .stream(
            &format!("{EVENTS_PATH}?after=later"),
            &[(PRINCIPAL_HEADER, Who::A1.login())],
        )
        .await;
    assert!(garbage.next().await.is_comment());
    let frame = garbage.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(META_INVALIDATE), "{frame:?}");
    drop((queried, both, garbage));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_guest_is_refused_and_a_bearer_token_gets_a_stream() {
    let harness = boot().await;
    let http = harness.http().await;
    let mut guest = http.events(Who::Guest, None).await;
    assert_eq!(guest.status(), 401);
    assert_eq!(
        guest.header("content-type"),
        Some("application/json"),
        "the refusal is an envelope"
    );
    let body = guest.json().await;
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["error"]["code"], json!("unauthenticated"));
    assert_eq!(body["error"]["key"], json!("keel.error.unauthenticated"));
    assert!(body["error"]["requestId"].is_string(), "{body}");

    let mut bearer = http
        .stream(
            EVENTS_PATH,
            &[("authorization", &format!("Bearer {}", Who::A1.login()))],
        )
        .await;
    assert_eq!(bearer.status(), 200);
    assert_eq!(bearer.header("content-type"), Some("text/event-stream"));
    assert_eq!(bearer.header("cache-control"), Some("no-store"));
    assert!(bearer.header("x-keel-request-id").is_some());
    let first = bearer.next().await;
    assert_eq!(first.retry, Some(3000));
    let seq = touch(&harness, Who::A1, "mine").await;
    let frame = bearer.next_event().await;
    assert_eq!(invalidate(&frame).0, seq);

    let mut unknown = http
        .stream(EVENTS_PATH, &[("authorization", "Bearer nobody")])
        .await;
    assert_eq!(unknown.status(), 401);
    assert_eq!(
        unknown.json().await["error"]["key"],
        json!("keel.error.unauthenticated")
    );
    drop((guest, bearer, unknown));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn live_frames_reach_the_addressed_principal_and_the_admins() {
    let harness = boot().await;
    let http = harness.http().await;
    let mut a1 = opened(&http, Who::A1, None).await;
    let mut a2 = opened(&http, Who::A2, None).await;
    let mut admin = opened(&http, Who::Admin, None).await;
    let mut owner = opened(&http, Who::Owner, None).await;
    let a1_principal = harness.principal(Who::A1).clone();
    let bus = live(&harness);
    bus.publish(frame(
        a1_principal.id,
        a1_principal.account_id,
        "task.progress",
        "progress",
    ));
    bus.publish(frame(
        a1_principal.id,
        a1_principal.account_id,
        "notification",
        "note",
    ));

    assert_eq!(
        live_event(&mut a1, "task.progress").await,
        json!({ "label": "progress" })
    );
    assert_eq!(
        live_event(&mut a1, "notification").await,
        json!({ "label": "note" })
    );
    assert_eq!(
        live_event(&mut admin, "task.progress").await,
        json!({ "label": "progress" })
    );
    assert_eq!(
        live_event(&mut admin, "notification").await,
        json!({ "label": "note" })
    );
    assert_eq!(
        live_event(&mut owner, "task.progress").await,
        json!({ "label": "progress" })
    );
    assert_eq!(
        live_event(&mut owner, "notification").await,
        json!({ "label": "note" })
    );
    assert!(a2.quiet(QUIET).await, "another user sees no frame of a1");
    assert!(a1.quiet(QUIET).await);
    drop((a1, a2, admin, owner));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn live_frames_of_an_account_reach_its_reseller_and_not_a_user_of_another_account() {
    let harness = boot().await;
    let http = harness.http().await;
    let mut reseller = opened(&http, Who::Reseller, None).await;
    let mut a2 = opened(&http, Who::A2, None).await;
    let mut unrelated = opened(&http, Who::Unrelated, None).await;
    let a1_principal = harness.principal(Who::A1).clone();
    let unrelated_principal = harness.principal(Who::Unrelated).clone();
    let bus = live(&harness);
    bus.publish(frame(
        a1_principal.id,
        a1_principal.account_id,
        "task.progress",
        "in the subtree",
    ));
    assert_eq!(
        live_event(&mut reseller, "task.progress").await,
        json!({ "label": "in the subtree" })
    );
    assert!(
        a2.quiet(QUIET).await,
        "a user of another account sees nothing"
    );
    assert!(unrelated.quiet(QUIET).await);

    bus.publish(frame(
        unrelated_principal.id,
        unrelated_principal.account_id,
        "task.progress",
        "outside the subtree",
    ));
    assert_eq!(
        live_event(&mut unrelated, "task.progress").await,
        json!({ "label": "outside the subtree" })
    );
    assert!(
        reseller.quiet(QUIET).await,
        "the reseller sees only its subtree"
    );
    assert!(a2.quiet(QUIET).await);
    drop((reseller, a2, unrelated));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn every_open_stream_closes_when_the_runtime_drains_so_stop_returns_within_a_second() {
    let harness = boot().await;
    let http = Http::new(
        harness.db().clone(),
        harness.readiness().clone(),
        HttpOptions::default(),
    );
    http.attach(harness.dispatcher().clone())
        .expect("the kernel attaches once");
    let server = TestHttp::bind(with_test_principal(http.router()))
        .await
        .expect("the server binds");
    let mut a1 = opened(&server, Who::A1, None).await;
    let mut reseller = opened(&server, Who::Reseller, None).await;
    let mut admin = opened(&server, Who::Admin, None).await;
    let seq = touch(&harness, Who::A1, "before the drain").await;
    assert_eq!(invalidate(&a1.next_event().await).0, seq);
    assert_eq!(invalidate(&admin.next_event().await).0, seq);
    assert_eq!(invalidate(&reseller.next_event().await).0, seq);
    tokio::time::timeout(PATIENCE, async {
        while http.open_streams() < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("three streams are counted");

    harness.readiness().draining();
    let stopped = tokio::time::timeout(STOP_WITHIN, server.stop()).await;
    assert!(
        matches!(stopped, Ok(Ok(()))),
        "the server stops within a second while streams are open: {stopped:?}"
    );
    for (who, stream) in [
        ("a1", &mut a1),
        ("reseller", &mut reseller),
        ("admin", &mut admin),
    ] {
        let last = stream.next().await;
        assert_eq!(last.retry, Some(3000), "{who}: {last:?}");
        assert!(
            last.comments.iter().any(|c| c == "draining"),
            "{who}: {last:?}"
        );
        assert!(
            stream.closes_within(PATIENCE).await,
            "{who}: the stream closes after the final frame"
        );
    }
    tokio::time::timeout(PATIENCE, async {
        while http.open_streams() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("every stream is released");
    harness.finish().await;
}

#[tokio::test]
async fn the_fifth_stream_of_a_session_closes_the_first_with_retry_zero() {
    let harness = boot_with_credentials().await;
    let http = harness.http().await;
    let session = cookie(&issue(&harness, Who::A1, "ks_"));
    let other = cookie(&issue(&harness, Who::A1, "ks_"));
    let mut neighbour = opened_with(&http, &[("cookie", &other)]).await;
    let mut streams = Vec::new();
    for _ in 0..=SSE_MAX_CLIENTS {
        streams.push(opened_with(&http, &[("cookie", &session)]).await);
    }
    let mut first = streams.remove(0);
    let last = first.next().await;
    assert_eq!(last.retry, Some(0), "{last:?}");
    assert!(last.comments.iter().any(|c| c == "superseded"), "{last:?}");
    assert!(
        first.closes_within(PATIENCE).await,
        "the first stream closes after the final frame"
    );

    let seq = touch(&harness, Who::A1, "after the fifth stream").await;
    for stream in streams.iter_mut().chain([&mut neighbour]) {
        assert_eq!(invalidate(&stream.next_event().await).0, seq);
    }
    drop((first, streams, neighbour));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_stream_opened_on_behalf_keeps_the_customer_at_each_keep_alive_until_the_delegation_is_refused()
 {
    let harness = boot_with_credentials().await;
    let http = harness
        .http_with(HttpOptions {
            sse_keep_alive: KEEP_ALIVE,
            ..HttpOptions::default()
        })
        .await;
    let token = issue(&harness, Who::Admin, "kpt_");
    let header = format!("as={}", Id::generate());
    DELEGATIONS
        .lock()
        .expect("the delegations")
        .insert(header.clone(), harness.principal(Who::A1).clone());
    let bearer = format!("Bearer {token}");
    let headers = [
        ("authorization", bearer.as_str()),
        (ON_BEHALF_OF, header.as_str()),
    ];
    let mut stream = opened_with(&http, &headers).await;
    for _ in 0..2 {
        let beat = stream.next().await;
        assert!(
            beat.comments.iter().any(|c| c == "keep-alive"),
            "a keep-alive passes while the delegation holds: {beat:?}"
        );
    }
    let other = touch(&harness, Who::A2, "of another account").await;
    let own = touch(&harness, Who::A1, "of the customer").await;
    assert!(other < own);
    assert_eq!(invalidate(&stream.next_event().await).0, own);

    DELEGATIONS.lock().expect("the delegations").remove(&header);
    assert!(
        stream.closes_within(PATIENCE).await,
        "a refused delegation loses its stream"
    );
    let refused = http.stream(EVENTS_PATH, &headers).await;
    assert_eq!(refused.status(), 404);
    let wide = opened_with(&http, &[("authorization", bearer.as_str())]).await;
    revoke(&token);
    drop((stream, refused, wide));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

async fn followed(http: &TestHttp, token: &str, on_behalf: Option<&str>) -> EventStream {
    let bearer = format!("Bearer {token}");
    let mut headers = vec![("authorization", bearer.as_str())];
    headers.extend(on_behalf.map(|header| (ON_BEHALF_OF, header)));
    let tail = http.stream(TAIL_PATH, &headers).await;
    assert_eq!(tail.status(), 200, "{headers:?}");
    tail
}

async fn keeps_alive(tail: &mut EventStream) {
    for _ in 0..2 {
        let beat = tail.next().await;
        assert!(
            beat.comments.iter().any(|c| c == "keep-alive"),
            "a keep-alive passes while the stream is admitted: {beat:?}"
        );
    }
}

async fn next_line(tail: &mut EventStream) -> Value {
    let line = tail.next_event().await;
    assert_eq!(line.event.as_deref(), Some(LINE_EVENT), "{line:?}");
    line.json()["text"].clone()
}

#[tokio::test]
async fn a_line_stream_opened_on_behalf_keeps_the_customer_until_the_delegation_is_refused() {
    let harness = boot_with_credentials().await;
    let http = harness
        .http_with(HttpOptions {
            sse_keep_alive: KEEP_ALIVE,
            ..HttpOptions::default()
        })
        .await;
    let customer = harness.principal(Who::A1).clone();
    let forbidden = format!("as={}", Id::generate());
    let missing = format!("as={}", Id::generate());
    for header in [&forbidden, &missing] {
        DELEGATIONS
            .lock()
            .expect("the delegations")
            .insert(header.clone(), customer.clone());
    }
    let mut delegated = Vec::new();
    for header in [&forbidden, &missing] {
        let token = issue(&harness, Who::Admin, "kpt_");
        delegated.push(followed(&http, &token, Some(header)).await);
    }
    let revoked = issue(&harness, Who::A2, "kat_");
    let refused_token = issue(&harness, Who::A2, "kat_");
    let mut plain = Vec::new();
    for token in [&revoked, &refused_token] {
        plain.push(followed(&http, token, None).await);
    }
    for tail in &mut delegated {
        assert_eq!(next_line(tail).await, json!(customer.login));
        keeps_alive(tail).await;
    }
    for tail in &mut plain {
        assert_eq!(next_line(tail).await, json!(Who::A2.login()));
        keeps_alive(tail).await;
    }
    TAIL.send("after the keep-alives".to_owned())
        .expect("the streams listen");
    for tail in delegated.iter_mut().chain(plain.iter_mut()) {
        assert_eq!(next_line(tail).await, json!("after the keep-alives"));
    }

    refuse(&forbidden);
    DELEGATIONS
        .lock()
        .expect("the delegations")
        .remove(&missing);
    revoke(&revoked);
    refuse(&refused_token);
    for tail in delegated.iter_mut().chain(plain.iter_mut()) {
        let end = tail.next_event().await;
        assert_eq!(end.event.as_deref(), Some(END_EVENT), "{end:?}");
        assert_eq!(end.json(), json!({ "reason": "expired" }));
        assert!(
            tail.closes_within(PATIENCE).await,
            "a stream that is no longer admitted ends"
        );
    }
    drop((delegated, plain));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn a_revoked_session_is_closed_at_the_next_keep_alive() {
    let harness = boot_with_credentials().await;
    let http = harness
        .http_with(HttpOptions {
            sse_keep_alive: KEEP_ALIVE,
            ..HttpOptions::default()
        })
        .await;
    let session = issue(&harness, Who::A1, "ks_");
    let token = issue(&harness, Who::A1, "kat_");
    let kept = issue(&harness, Who::A1, "ks_");
    let bearer = format!("Bearer {token}");
    let mut by_session = opened_with(&http, &[("cookie", &cookie(&session))]).await;
    let mut by_token = opened_with(&http, &[("authorization", &bearer)]).await;
    let mut unrevoked = opened_with(&http, &[("cookie", &cookie(&kept))]).await;
    for stream in [&mut by_session, &mut by_token, &mut unrevoked] {
        let beat = stream.next().await;
        assert!(
            beat.comments.iter().any(|c| c == "keep-alive"),
            "a keep-alive passes while the credential holds: {beat:?}"
        );
    }

    revoke(&session);
    revoke(&token);
    assert!(
        by_session.closes_within(PATIENCE).await,
        "the revoked session loses its stream"
    );
    assert!(
        by_token.closes_within(PATIENCE).await,
        "the revoked token loses its stream"
    );
    let seq = touch(&harness, Who::A1, "after the revocation").await;
    assert_eq!(invalidate(&unrevoked.next_event().await).0, seq);
    revoke(&kept);
    drop((by_session, by_token, unrevoked));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
