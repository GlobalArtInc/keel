use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use futures::{StreamExt, stream};
use keel_kernel::dispatcher::STREAM_ROUTE_KEY;
use keel_kernel::http::{END_EVENT, LINE_EVENT};
use keel_sdk::prelude::*;
use keel_sdk::system::{AuditEvent, AuditSink};
use keel_sdk::{Registrar, migrator};
use keel_testkit::{Harness, PRINCIPAL_HEADER, Who};
use keel_types::{Envelope, ErrorCode};
use sea_orm::{ActiveModelTrait, Set};
use tokio::sync::broadcast;

const ROUTE: &str = "/api/v1/x/desk/card.watch";
const FUNCTION: &str = "desk.card.watch";
const BYE: &str = "bye";

mod card {
    use keel_sdk::Owned;
    use keel_sdk::schemars::JsonSchema;
    use keel_sdk::sea_orm::entity::prelude::*;
    use keel_sdk::serde::Serialize;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, JsonSchema)]
    #[sea_orm(table_name = "desk_card")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub account_id: String,
        pub title: String,
        pub color: String,
        pub priority: i32,
        pub pin: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    impl Owned for Entity {
        fn account_column() -> Self::Column {
            Column::AccountId
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pick {
    id: Id,
}

#[derive(Default)]
struct Recorder(Mutex<Vec<AuditEvent>>);

impl AuditSink for Recorder {
    fn record(&self, event: AuditEvent) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Recorder {
    fn of(&self, request_id: &str) -> Vec<AuditEvent> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.function == FUNCTION && event.request_id == request_id)
            .cloned()
            .collect()
    }
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);
static FEED: LazyLock<broadcast::Sender<String>> = LazyLock::new(|| broadcast::channel(16).0);

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/kernel/modules/desk/migrations")
        .table("keel_migration_desk");

static DESK: ModuleDef = ModuleDef {
    id: "desk",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.stream(
        function::<Pick, LogLine>(FUNCTION)
            .level(Level::User)
            .owned::<card::Entity>(),
        watch,
    )
    .provide::<dyn AuditSink>(RECORDER.clone());
    Ok(())
}

async fn watch(call: Call<Pick>) -> Result<Lines> {
    let first = LogLine::new(
        LogLevel::Info,
        format!("{} watches {}", call.principal().login, call.params().id),
    );
    let live = stream::unfold(FEED.subscribe(), |mut feed| async move {
        match feed.recv().await {
            Ok(text) if text != BYE => Some((LogLine::new(LogLevel::Warn, text), feed)),
            _ => None,
        }
    });
    Ok(Lines::new(stream::once(async { first }).chain(live)))
}

async fn card_of(harness: &Harness, who: Who) -> Id {
    let id = Id::generate();
    card::ActiveModel {
        id: Set(id.to_string()),
        account_id: Set(harness.principal(who).account_id.to_string()),
        title: Set("inbox".to_owned()),
        color: Set("white".to_owned()),
        priority: Set(0),
        pin: Set(String::new()),
    }
    .insert(harness.db())
    .await
    .expect("a card");
    id
}

#[keel_testkit::tokio::test]
async fn a_stream_serves_lines_as_sse_behind_auth_and_audits_the_open() {
    let harness = Harness::new().module(&DESK).boot().await;
    let card = card_of(&harness, Who::A1).await;
    let http = harness.http().await;
    let path = format!("{ROUTE}?id={card}");

    let guest = http.get(Who::Guest, &path).await;
    assert_eq!(guest.status, 401, "{}", guest.body);
    assert_eq!(
        guest.json()["error"]["key"],
        json!("keel.error.unauthenticated")
    );

    let posted = http.post(Who::A1, ROUTE, &json!({ "id": card })).await;
    assert_eq!(posted.status, 422, "{}", posted.body);
    assert_eq!(posted.header("allow"), Some("GET"));

    let mut tail = http.stream(&path, &[(PRINCIPAL_HEADER, "a1")]).await;
    assert_eq!(tail.status(), 200);
    assert!(
        tail.header("content-type")
            .is_some_and(|value| value.starts_with("text/event-stream")),
        "{:?}",
        tail.header("content-type")
    );
    assert_eq!(tail.header("cache-control"), Some("no-store"));
    let request_id = tail
        .header("x-keel-request-id")
        .expect("the request id")
        .to_owned();

    let first = tail.next_event().await;
    assert_eq!(first.event.as_deref(), Some(LINE_EVENT));
    assert_eq!(
        first.json(),
        json!({ "level": "info", "text": format!("a1 watches {card}") })
    );

    let audited = RECORDER.of(&request_id);
    assert_eq!(audited.len(), 1, "the open is audited once");
    assert_eq!(audited[0].mode, Mode::Read);
    assert_eq!(audited[0].principal.login, "a1");
    assert_eq!(audited[0].params, json!({ "id": card }));
    assert!(audited[0].outcome.is_committed());

    FEED.send("the disk is almost full".to_owned())
        .expect("the stream listens");
    let live = tail.next_event().await;
    assert_eq!(live.event.as_deref(), Some(LINE_EVENT));
    assert_eq!(
        live.json(),
        json!({ "level": "warn", "text": "the disk is almost full" })
    );

    FEED.send(BYE.to_owned()).expect("the stream listens");
    let end = tail.next_event().await;
    assert_eq!(end.event.as_deref(), Some(END_EVENT));
    assert_eq!(end.json(), json!({ "reason": "done" }));

    let call_route = http
        .post(
            Who::A1,
            &format!("/api/v1/call/{FUNCTION}"),
            &json!({ "id": card }),
        )
        .await;
    assert_eq!(call_route.status, 422, "{}", call_route.body);
    assert_eq!(call_route.json()["error"]["key"], json!(STREAM_ROUTE_KEY));
    assert_eq!(call_route.json()["error"]["params"]["route"], json!(ROUTE));

    let socket = harness.socket().await;
    match socket.call(FUNCTION, json!({ "id": card })).await {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(error.key, STREAM_ROUTE_KEY);
        }
        Envelope::Ok { data, .. } => panic!("the socket opened a stream: {data}"),
    }
    socket.stop().await.expect("the socket stops");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_stream_of_another_account_is_refused() {
    let harness = Harness::new().module(&DESK).boot().await;
    let card = card_of(&harness, Who::A1).await;
    let http = harness.http().await;

    let mut refused = http
        .stream(&format!("{ROUTE}?id={card}"), &[(PRINCIPAL_HEADER, "a2")])
        .await;
    assert_eq!(refused.status(), 404);
    let envelope = refused.json().await;
    assert_eq!(envelope["ok"], json!(false));
    assert_eq!(envelope["error"]["code"], json!("not_found"));

    let mut reseller = http
        .stream(
            &format!("{ROUTE}?id={card}"),
            &[(PRINCIPAL_HEADER, Who::Reseller.login())],
        )
        .await;
    assert_eq!(reseller.status(), 200, "the reseller above a1 may watch");
    assert_eq!(
        reseller.next_event().await.event.as_deref(),
        Some(LINE_EVENT)
    );
    drop(reseller);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
