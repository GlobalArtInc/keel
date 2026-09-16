use std::sync::Arc;
use std::time::Duration;

use keel_kernel::dispatcher::OUTBOX_BATCH;
use keel_kernel::http::{DATA_INVALIDATE, LAST_EVENT_ID, META_INVALIDATE, SSE_RETRY};
use keel_kernel::{Http, HttpOptions};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{Frame, LiveEvents, OutboxSubscriber};
use keel_testkit::http::with_test_principal;
use keel_testkit::{EVENTS_PATH, EventStream, Harness, PRINCIPAL_HEADER, SseFrame, TestHttp, Who};
use keel_types::Event;

const QUIET: Duration = Duration::from_millis(700);
const PATIENCE: Duration = Duration::from_secs(5);
const STOP_WITHIN: Duration = Duration::from_secs(1);

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
    );
    Ok(())
}

async fn boot() -> Harness {
    Harness::new().module(&STREAM).boot().await
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
    assert_eq!(events.len(), 2);
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
