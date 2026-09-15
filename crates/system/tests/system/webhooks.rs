use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::db::Db;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::serde_json;
use keel_sdk::system::SecretCipher;
use keel_sdk::types::Event;
use keel_system::webhooks::{
    CURSOR, DEAD_LETTER, DELIVERY_LIST, DELIVERY_RETRY, ERROR_MASK, ERROR_NOT_RETRYABLE,
    ERROR_SECRET_SHORT, ERROR_URL_PRIVATE, EVENT_ID_HEADER, FUNCTIONS, LADDER, MAX_ATTEMPTS,
    MENU_PATH, MODULE, PRIVATE_TARGETS_KEY, Resolver, SECRET_GENERATED_BYTES, SECRET_MIN_BYTES,
    SIGNATURE_HEADER, STATE_DEAD, STATE_DELIVERED, STATE_FAILED, STATE_PENDING,
    SUBSCRIPTION_DELETE, SUBSCRIPTION_GET, SUBSCRIPTION_LIST, SUBSCRIPTION_SET,
    SUBSCRIPTION_TOGGLE, base64url, drop_account, verify,
};
use keel_testkit::{Fixture, Harness, Reply, Who, tokio};
use tokio::net::TcpListener;

const PATIENCE: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(700);
const START: &str = "2026-09-16T10:00:00Z";
const PUBLIC: &str = "93.184.216.34";
const EVENT_SAVED: &str = "probe.note.saved";
const EVENT_TOUCHED: &str = "probe.note.touched";
const MASK_NOTES: &str = "probe.note.*";
const MORE_THAN_A_BATCH: usize = 55;

type Answers = HashMap<String, VecDeque<Vec<IpAddr>>>;

static PRIVATE_TARGETS: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);
static ANSWERS: Mutex<Option<HashMap<String, Answers>>> = Mutex::new(None);

async fn database_name(db: &Db) -> Result<String> {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT current_database() AS name",
        ))
        .await?
        .ok_or_else(|| KeelError::internal("probe.error.no_database"))?;
    Ok(row.try_get::<String>("", "name")?)
}

fn allow_private(harness: &Harness, allowed: bool) {
    PRIVATE_TARGETS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(harness.database().name().to_owned(), allowed);
}

fn answer(harness: &Harness, host: &str, ips: &[&str]) {
    ANSWERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .entry(harness.database().name().to_owned())
        .or_default()
        .entry(host.to_owned())
        .or_default()
        .push_back(ips.iter().map(|ip| ip.parse().expect("an ip")).collect());
}

struct Toggle;

#[async_trait]
impl SettingsSource for Toggle {
    async fn resolve(&self, key: &str, _: Id, db: &Db) -> Result<Option<Value>> {
        if key != PRIVATE_TARGETS_KEY {
            return Ok(None);
        }
        let name = database_name(db).await?;
        let allowed = PRIVATE_TARGETS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|map| map.get(&name).copied())
            .unwrap_or(true);
        Ok(Some(json!(allowed)))
    }
}

struct Scripted {
    database: Mutex<Option<String>>,
}

#[async_trait]
impl Resolver for Scripted {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        let database = self
            .database
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| KeelError::internal("probe.error.resolver_unbound"))?;
        let mut answers = ANSWERS.lock().unwrap_or_else(PoisonError::into_inner);
        let queue = answers
            .as_mut()
            .and_then(|map| map.get_mut(&database))
            .and_then(|hosts| hosts.get_mut(host))
            .ok_or_else(|| {
                KeelError::validation("webhook.error.url_unresolved")
                    .field("url")
                    .param("host", host)
                    .param("reason", "no scripted answer")
            })?;
        let ips = if queue.len() > 1 {
            queue.pop_front().unwrap_or_default()
        } else {
            queue.front().cloned().unwrap_or_default()
        };
        Ok(ips
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect())
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Save {
    id: Id,
    #[serde(default)]
    account: Option<Id>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["webhook"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let scripted = Arc::new(Scripted {
        database: Mutex::new(None),
    });
    let bound = scripted.clone();
    r.provide::<dyn SettingsSource>(Arc::new(Toggle))
        .provide::<dyn Resolver>(scripted)
        .function(
            function::<Save, Nothing>("probe.note.save")
                .kind(Kind::Action)
                .level(Level::User)
                .unowned()
                .key("id")
                .handler(|call: Call<Save>| async move {
                    let account = call.resolve_account(call.params().account).await?;
                    call.emit(
                        EVENT_SAVED,
                        &json!({ "id": call.params().id, "account": account }),
                    )?;
                    Ok(Nothing {})
                }),
        )
        .function(
            function::<Nothing, Nothing>("probe.note.touch")
                .kind(Kind::Action)
                .level(Level::User)
                .unowned()
                .handler(|call: Call<Nothing>| async move {
                    call.emit(EVENT_TOUCHED, &json!({}))?;
                    Ok(Nothing {})
                }),
        )
        .start(move |ctx| async move {
            let name = database_name(&keel_sdk::system::ContextExt::db(&ctx)).await?;
            *bound
                .database
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(name);
            Ok(())
        });
    Ok(())
}

#[derive(Clone, Debug)]
struct Received {
    hook: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }

    fn event(&self) -> Event {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the body is not an event: {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn t(&self) -> i64 {
        self.header(SIGNATURE_HEADER)
            .split(',')
            .find_map(|part| part.strip_prefix("t="))
            .and_then(|t| t.parse().ok())
            .expect("a t in the signature")
    }
}

#[derive(Clone, Debug)]
enum Answer {
    Status(u16),
    Redirect(String),
}

#[derive(Clone)]
struct Hooks {
    received: Arc<Mutex<Vec<Received>>>,
    answers: Arc<Mutex<HashMap<String, Answer>>>,
}

async fn hook(
    State(hooks): State<Hooks>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    hooks
        .received
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(Received {
            hook: name.clone(),
            headers,
            body: body.to_vec(),
        });
    let answer = hooks
        .answers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&name)
        .cloned();
    match answer {
        Some(Answer::Redirect(to)) => (StatusCode::FOUND, [(header::LOCATION, to)]).into_response(),
        Some(Answer::Status(status)) => {
            { StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR) }
                .into_response()
        }
        None => StatusCode::OK.into_response(),
    }
}

struct Listener {
    port: u16,
    hooks: Hooks,
}

impl Listener {
    async fn start() -> Self {
        let hooks = Hooks {
            received: Arc::new(Mutex::new(Vec::new())),
            answers: Arc::new(Mutex::new(HashMap::new())),
        };
        let router = Router::new()
            .route("/hook/{name}", post(hook))
            .with_state(hooks.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port is free");
        let port = listener.local_addr().expect("an address").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { port, hooks }
    }

    fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/hook/{name}", self.port)
    }

    fn answer(&self, name: &str, answer: Answer) {
        self.hooks
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), answer);
    }

    fn received(&self, name: &str) -> Vec<Received> {
        self.hooks
            .received
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|received| received.hook == name)
            .cloned()
            .collect()
    }
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

async fn boot() -> Harness {
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .clock(at(START))
        .boot()
        .await;
    allow_private(&harness, true);
    harness
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused_on(reply: &Reply, path: &str, key: &str) -> Value {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), 422, "{error:?}");
    let field = error
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap_or_else(|| panic!("no field {path} in {error:?}"));
    assert_eq!(field.key.as_deref(), Some(key), "{error:?}");
    field.params.clone().into()
}

async fn wait_until<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn subscribe(harness: &Harness, who: Who, name: &str, url: &str, masks: &[&str]) -> Value {
    let reply = harness
        .call(
            who,
            SUBSCRIPTION_SET,
            json!({ "name": name, "url": url, "events": masks }),
        )
        .await;
    data(&reply).clone()
}

async fn save(harness: &Harness, who: Who) -> Id {
    let id = Id::generate();
    let reply = harness
        .call(who, "probe.note.save", json!({ "id": id }))
        .await;
    data(&reply);
    id
}

async fn principal_in(harness: &Harness, login: &str, level: Level, account: Id) -> Principal {
    let reply = harness
        .call(
            Who::Owner,
            "keel.principal.create",
            json!({ "login": login, "level": level, "account": account }),
        )
        .await;
    let id = data(&reply)["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("an id");
    Principal {
        id,
        login: login.to_owned(),
        level,
        account_id: account,
        ..harness.principal(Who::A1).clone()
    }
}

async fn deliveries(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT subscription_id || ' ' || type || ' ' || state || ' ' || attempt::text FROM webhook_delivery ORDER BY created_at, id")
        .await
}

async fn delivery_rows(harness: &Harness, who: Who, scope: Value) -> Vec<Value> {
    let listed = harness
        .call(
            who,
            DELIVERY_LIST,
            json!({ "scope": scope, "sort": { "key": "createdAt", "dir": "asc" } }),
        )
        .await;
    data(&listed)["data"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn delivery_of(harness: &Harness, subscription: &str) -> Option<Value> {
    delivery_rows(
        harness,
        Who::Owner,
        json!({ "subscription_id": subscription }),
    )
    .await
    .into_iter()
    .next()
}

async fn wait_attempt(harness: &Harness, subscription: &str, attempt: i64) -> Value {
    wait_until(&format!("attempt {attempt} of {subscription}"), || async {
        delivery_of(harness, subscription)
            .await
            .is_some_and(|row| row["attempt"].as_i64() >= Some(attempt))
    })
    .await;
    delivery_of(harness, subscription)
        .await
        .expect("the delivery appeared")
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT subject || ' ' || coalesce(account_id, '-') || ' ' || count::text FROM notify_problem WHERE key = '{DEAD_LETTER}' AND cleared_at IS NULL ORDER BY raised_at"
        ))
        .await
}

struct SubscriptionFixture;

#[async_trait]
impl Fixture for SubscriptionFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let reply = harness
            .call(
                Who::Owner,
                SUBSCRIPTION_SET,
                json!({
                    "name": format!("of {account}"),
                    "url": format!("http://127.0.0.1:9/hook/{account}"),
                    "events": [MASK_NOTES],
                    "account": account
                }),
            )
            .await;
        data(&reply)["id"].as_str().expect("an id").to_owned()
    }
}

#[tokio::test]
async fn a_subscription_receives_a_signed_delivery_for_a_matching_event_and_the_signature_verifies()
{
    let harness = boot().await;
    let listener = Listener::start().await;
    let created = subscribe(
        &harness,
        Who::A1,
        "a1 hooks",
        &listener.url("a1"),
        &[MASK_NOTES],
    )
    .await;
    let subscription = created["id"].as_str().expect("an id").to_owned();
    let secret = created["secret"].as_str().expect("a generated secret");
    assert_eq!(secret.len(), 43);
    assert_eq!(created["enabled"], json!(true));
    assert_eq!(created["events"], json!([MASK_NOTES]));
    assert_eq!(created["createdBy"], json!("a1"));
    assert!(created.get("accountId").is_none(), "{created}");
    let note = save(&harness, Who::A1).await;
    wait_until("the delivery reaches the listener", || async {
        !listener.received("a1").is_empty()
    })
    .await;
    let received = listener.received("a1");
    assert_eq!(received.len(), 1);
    let hit = &received[0];
    assert_eq!(hit.header("content-type"), "application/json");
    let event = hit.event();
    assert_eq!(hit.header(EVENT_ID_HEADER), event.id.to_string());
    assert_eq!(event.kind, EVENT_SAVED);
    assert_eq!(event.module, "probe");
    assert_eq!(event.data["id"], json!(note));
    let subject = event
        .subject
        .as_ref()
        .expect("the dispatcher derives a subject");
    assert_eq!(subject.kind, "probe.note");
    assert_eq!(subject.id, note);
    assert_eq!(subject.account_id, Some(harness.fixtures().accounts.a1));
    assert_eq!(event.instance_id, harness.instance());
    let now = harness.clock().now().timestamp();
    assert_eq!(hit.t(), now);
    assert_eq!(
        verify(
            secret.as_bytes(),
            hit.header(SIGNATURE_HEADER),
            &hit.body,
            now
        ),
        Ok(())
    );
    assert!(
        verify(
            b"another secret of sixteen",
            hit.header(SIGNATURE_HEADER),
            &hit.body,
            now
        )
        .is_err()
    );
    wait_until("the delivery is marked delivered", || async {
        delivery_of(&harness, &subscription)
            .await
            .is_some_and(|row| row["state"] == json!(STATE_DELIVERED))
    })
    .await;
    let row = delivery_of(&harness, &subscription)
        .await
        .expect("the delivery row");
    assert_eq!(row["attempt"], json!(1));
    assert_eq!(row["lastStatus"], json!(200));
    assert_eq!(row["lastError"], Value::Null);
    assert_eq!(row["type"], json!(EVENT_SAVED));
    assert_eq!(row["eventId"], json!(event.id));
    assert!(row["deliveredAt"].is_string(), "{row}");
    assert_eq!(row["accountId"], json!(harness.fixtures().accounts.a1));
    let own = delivery_rows(&harness, Who::A1, json!({})).await;
    assert_eq!(own.len(), 1);
    assert_eq!(own[0]["id"], row["id"]);
    assert!(own[0].get("accountId").is_none(), "{}", own[0]);
    let other = delivery_rows(
        &harness,
        Who::A1,
        json!({ "subscription_id": Id::generate() }),
    )
    .await;
    assert!(other.is_empty());
    let cursor = harness
        .database()
        .rows(&format!(
            "SELECT seq::text FROM webhook_cursor WHERE consumer = '{CURSOR}'"
        ))
        .await;
    let newest = harness
        .database()
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    assert_eq!(cursor, newest);
    harness.finish().await;
}

#[tokio::test]
async fn an_event_outside_the_masks_or_outside_the_subscribers_scope_creates_no_delivery() {
    let harness = boot().await;
    let listener = Listener::start().await;
    let a1 = subscribe(
        &harness,
        Who::A1,
        "a1 notes",
        &listener.url("a1"),
        &[MASK_NOTES],
    )
    .await;
    let admin = subscribe(
        &harness,
        Who::Admin,
        "admin notes",
        &listener.url("admin"),
        &["probe.note.saved", EVENT_TOUCHED],
    )
    .await;
    let a2 = subscribe(
        &harness,
        Who::A2,
        "a2 other",
        &listener.url("a2"),
        &["probe.other.*"],
    )
    .await;
    let reseller = subscribe(
        &harness,
        Who::Reseller,
        "reseller notes",
        &listener.url("reseller"),
        &["probe.*"],
    )
    .await;
    let from_a1 = save(&harness, Who::A1).await;
    let from_a2 = save(&harness, Who::A2).await;
    let from_unrelated = save(&harness, Who::Unrelated).await;
    let touched = harness
        .call(Who::Admin, "probe.note.touch", json!({}))
        .await;
    data(&touched);
    wait_until("the admin receives the three events", || async {
        listener.received("admin").len() >= 3
    })
    .await;
    wait_until("the reseller receives the subtree's events", || async {
        listener.received("reseller").len() >= 2
    })
    .await;
    tokio::time::sleep(QUIET).await;
    let kinds = |name: &str| -> Vec<(String, Option<Id>)> {
        listener
            .received(name)
            .iter()
            .map(|hit| {
                let event = hit.event();
                (
                    event.kind,
                    event.subject.and_then(|subject| subject.account_id),
                )
            })
            .collect()
    };
    let accounts = &harness.fixtures().accounts;
    assert_eq!(
        kinds("a1"),
        [(EVENT_SAVED.to_owned(), Some(accounts.a1))],
        "a1's subscription receives a1's event alone"
    );
    let mut admin_seen = kinds("admin");
    admin_seen.sort();
    let mut admin_expected = vec![
        (EVENT_SAVED.to_owned(), Some(accounts.a1)),
        (EVENT_SAVED.to_owned(), Some(accounts.a2)),
        (EVENT_SAVED.to_owned(), Some(accounts.unrelated)),
        (EVENT_TOUCHED.to_owned(), None),
    ];
    admin_expected.sort();
    assert_eq!(admin_seen.len(), 4, "{admin_seen:?}");
    assert_eq!(admin_seen, admin_expected);
    let mut reseller_seen = kinds("reseller");
    reseller_seen.sort();
    assert_eq!(
        reseller_seen,
        [
            (EVENT_SAVED.to_owned(), Some(accounts.a1)),
            (EVENT_SAVED.to_owned(), Some(accounts.a2)),
        ],
        "the reseller's subscription receives its subtree and not the unrelated account or an event without a subject"
    );
    assert!(listener.received("a2").is_empty(), "the masks do not match");
    let a1_id = a1["id"].as_str().expect("id");
    let admin_id = admin["id"].as_str().expect("id");
    let a2_id = a2["id"].as_str().expect("id");
    let reseller_id = reseller["id"].as_str().expect("id");
    let rows = deliveries(&harness).await;
    assert_eq!(rows.iter().filter(|row| row.starts_with(a1_id)).count(), 1);
    assert_eq!(
        rows.iter().filter(|row| row.starts_with(admin_id)).count(),
        4
    );
    assert_eq!(rows.iter().filter(|row| row.starts_with(a2_id)).count(), 0);
    assert_eq!(
        rows.iter()
            .filter(|row| row.starts_with(reseller_id))
            .count(),
        2
    );
    assert_eq!(rows.len(), 7, "{rows:?}");
    let events: Vec<Id> = listener
        .received("admin")
        .iter()
        .filter_map(|hit| {
            hit.event().data["id"]
                .as_str()
                .and_then(|id| id.parse().ok())
        })
        .collect();
    for id in [from_a1, from_a2, from_unrelated] {
        assert!(events.contains(&id), "{id} reached the admin");
    }
    harness.finish().await;
}

#[tokio::test]
async fn a_subscription_placed_in_another_account_receives_that_accounts_events_alone() {
    let harness = boot().await;
    let listener = Listener::start().await;
    let accounts = &harness.fixtures().accounts;
    let admin = harness.principal(Who::Admin).id;
    let placed = harness
        .call(
            Who::Admin,
            SUBSCRIPTION_SET,
            json!({ "name": "placed in a1", "url": listener.url("placed"), "events": [MASK_NOTES], "account": accounts.a1 }),
        )
        .await;
    let placed = data(&placed).clone();
    let id = placed["id"].as_str().expect("an id").to_owned();
    assert_eq!(placed["accountId"], json!(accounts.a1));
    assert_eq!(placed["principalId"], json!(admin));
    let repointed = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "id": id, "name": "repointed by a1", "url": listener.url("repointed"), "events": [MASK_NOTES] }),
        )
        .await;
    assert_eq!(data(&repointed)["url"], json!(listener.url("repointed")));
    let kept = harness
        .call(Who::Admin, SUBSCRIPTION_GET, json!({ "id": id }))
        .await;
    assert_eq!(
        data(&kept)["principalId"],
        json!(harness.principal(Who::A1).id),
        "the subscriber is whoever last set the target"
    );
    assert_eq!(data(&kept)["accountId"], json!(accounts.a1));
    save(&harness, Who::A2).await;
    save(&harness, Who::Unrelated).await;
    let touched = harness
        .call(Who::Admin, "probe.note.touch", json!({}))
        .await;
    data(&touched);
    let own = save(&harness, Who::A1).await;
    wait_until("a1's event reaches the repointed url", || async {
        !listener.received("repointed").is_empty()
    })
    .await;
    tokio::time::sleep(QUIET).await;
    let hits = listener.received("repointed");
    assert_eq!(hits.len(), 1, "a1's event alone");
    let event = hits[0].event();
    assert_eq!(event.kind, EVENT_SAVED);
    assert_eq!(event.data["id"], json!(own));
    assert_eq!(
        event.subject.and_then(|subject| subject.account_id),
        Some(accounts.a1)
    );
    assert!(listener.received("placed").is_empty());
    let rows = deliveries(&harness).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].starts_with(&id), "{rows:?}");
    let listed = delivery_rows(&harness, Who::A1, json!({ "subscription_id": id })).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["type"], json!(EVENT_SAVED));
    harness.finish().await;
}

#[tokio::test]
async fn a_user_of_the_resellers_account_who_repoints_its_subscription_receives_that_accounts_events_alone()
 {
    let harness = boot().await;
    let listener = Listener::start().await;
    let accounts = &harness.fixtures().accounts;
    let staff = principal_in(&harness, "reseller-staff", Level::User, accounts.reseller).await;
    let placed = subscribe(
        &harness,
        Who::Reseller,
        "reseller notes",
        &listener.url("reseller"),
        &[MASK_NOTES],
    )
    .await;
    let id = placed["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        placed["principalId"],
        json!(harness.principal(Who::Reseller).id)
    );
    let repointed = harness
        .call_as_principal(
            &staff,
            SUBSCRIPTION_SET,
            json!({ "id": id, "name": "repointed by staff", "url": listener.url("staff"), "events": [MASK_NOTES] }),
        )
        .await;
    assert_eq!(data(&repointed)["url"], json!(listener.url("staff")));
    let kept = harness
        .call(Who::Reseller, SUBSCRIPTION_GET, json!({ "id": id }))
        .await;
    assert_eq!(data(&kept)["principalId"], json!(staff.id));
    assert_eq!(data(&kept)["accountId"], json!(accounts.reseller));
    save(&harness, Who::A1).await;
    save(&harness, Who::A2).await;
    save(&harness, Who::Unrelated).await;
    let touched = harness
        .call(Who::Admin, "probe.note.touch", json!({}))
        .await;
    data(&touched);
    let own = save(&harness, Who::Reseller).await;
    wait_until(
        "the reseller account's event reaches the staff url",
        || async { !listener.received("staff").is_empty() },
    )
    .await;
    tokio::time::sleep(QUIET).await;
    let hits = listener.received("staff");
    assert_eq!(hits.len(), 1, "the reseller account's own event alone");
    let event = hits[0].event();
    assert_eq!(event.kind, EVENT_SAVED);
    assert_eq!(event.data["id"], json!(own));
    assert_eq!(
        event.subject.and_then(|subject| subject.account_id),
        Some(accounts.reseller)
    );
    assert!(listener.received("reseller").is_empty());
    let rows = deliveries(&harness).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].starts_with(&id), "{rows:?}");
    harness.finish().await;
}

#[tokio::test]
async fn the_due_deliveries_of_a_disabled_subscription_do_not_hold_up_another_subscription() {
    let harness = boot().await;
    let listener = Listener::start().await;
    listener.answer("parked", Answer::Status(500));
    let parked = subscribe(
        &harness,
        Who::Admin,
        "parked",
        &listener.url("parked"),
        &[MASK_NOTES],
    )
    .await;
    let parked_id = parked["id"].as_str().expect("an id").to_owned();
    for _ in 0..MORE_THAN_A_BATCH {
        save(&harness, Who::Admin).await;
    }
    let parked_rows = || async {
        deliveries(&harness)
            .await
            .into_iter()
            .filter(|row| row.starts_with(&parked_id))
            .collect::<Vec<String>>()
    };
    wait_until(
        "every delivery of the parked subscription failed once",
        || async {
            let rows = parked_rows().await;
            rows.len() == MORE_THAN_A_BATCH
                && rows
                    .iter()
                    .all(|row| row.ends_with(&format!(" {STATE_FAILED} 1")))
        },
    )
    .await;
    assert_eq!(listener.received("parked").len(), MORE_THAN_A_BATCH);
    let disabled = harness
        .call(
            Who::Admin,
            SUBSCRIPTION_TOGGLE,
            json!({ "id": parked_id, "enabled": false }),
        )
        .await;
    assert_eq!(data(&disabled)["enabled"], json!(false));
    let due = at(START) + keel_sdk::chrono::Duration::from_std(LADDER[0]).expect("a delta");
    harness.clock().set(due);

    let live = subscribe(
        &harness,
        Who::Admin,
        "live",
        &listener.url("live"),
        &[MASK_NOTES],
    )
    .await;
    let live_id = live["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    wait_until("the live subscription's delivery is delivered", || async {
        delivery_of(&harness, &live_id)
            .await
            .is_some_and(|row| row["state"] == json!(STATE_DELIVERED))
    })
    .await;
    assert_eq!(listener.received("live").len(), 1);
    assert_eq!(
        listener.received("parked").len(),
        MORE_THAN_A_BATCH,
        "a disabled subscription is not posted"
    );
    let rows = parked_rows().await;
    assert_eq!(rows.len(), MORE_THAN_A_BATCH);
    assert!(
        rows.iter()
            .all(|row| row.ends_with(&format!(" {STATE_FAILED} 1"))),
        "{rows:?}"
    );

    let enabled = harness
        .call(
            Who::Admin,
            SUBSCRIPTION_TOGGLE,
            json!({ "id": parked_id, "enabled": true }),
        )
        .await;
    assert_eq!(data(&enabled)["enabled"], json!(true));
    wait_until(
        "the parked deliveries resume once enabled again",
        || async {
            parked_rows()
                .await
                .iter()
                .all(|row| row.ends_with(&format!(" {STATE_FAILED} 2")))
        },
    )
    .await;
    assert_eq!(listener.received("parked").len(), 2 * MORE_THAN_A_BATCH);
    harness.finish().await;
}

#[tokio::test]
async fn a_failed_delivery_follows_the_ladder_and_its_fifth_failure_is_a_problem_a_retry_clears() {
    let harness = boot().await;
    let listener = Listener::start().await;
    listener.answer("failing", Answer::Status(500));
    let created = subscribe(
        &harness,
        Who::Admin,
        "failing",
        &listener.url("failing"),
        &[MASK_NOTES],
    )
    .await;
    let subscription = created["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    let mut now = at(START);
    let mut seen_t = Vec::new();
    for attempt in 1..MAX_ATTEMPTS {
        let row = wait_attempt(&harness, &subscription, i64::from(attempt)).await;
        assert_eq!(row["attempt"], json!(attempt), "{row}");
        assert_eq!(row["state"], json!(STATE_FAILED), "{row}");
        assert_eq!(row["lastStatus"], json!(500), "{row}");
        let error = row["lastError"].as_str().expect("an error text");
        assert!(
            error.starts_with("webhook.error.status: 500"),
            "attempt {attempt}: {error}"
        );
        let wait = LADDER[usize::try_from(attempt - 1).expect("an index")];
        let expected = now + keel_sdk::chrono::Duration::from_std(wait).expect("a delta");
        assert_eq!(row["nextAt"], json!(expected), "attempt {attempt}");
        let hits = listener.received("failing");
        assert_eq!(hits.len(), usize::try_from(attempt).expect("a count"));
        let t = hits[hits.len() - 1].t();
        assert_eq!(
            t,
            now.timestamp(),
            "attempt {attempt} carries the time of its attempt"
        );
        seen_t.push(t);
        tokio::time::sleep(QUIET).await;
        assert_eq!(
            delivery_of(&harness, &subscription).await.expect("row")["attempt"],
            json!(attempt),
            "no attempt before the clock reaches nextAt"
        );
        now = expected;
        harness.clock().set(now);
    }
    let row = wait_attempt(&harness, &subscription, i64::from(MAX_ATTEMPTS)).await;
    assert_eq!(row["state"], json!(STATE_DEAD));
    assert_eq!(row["attempt"], json!(MAX_ATTEMPTS));
    seen_t.push(listener.received("failing").last().expect("a hit").t());
    assert!(
        seen_t.windows(2).all(|pair| pair[0] < pair[1]),
        "each retry carries a later t: {seen_t:?}"
    );
    let admin_account = harness.principal(Who::Admin).account_id;
    wait_until("the dead letter is raised", || async {
        !open_problems(&harness).await.is_empty()
    })
    .await;
    assert_eq!(
        open_problems(&harness).await,
        [format!("{subscription} {admin_account} 1")]
    );
    let problems = harness
        .database()
        .rows(&format!(
            "SELECT severity || ' ' || source || ' ' || (params_json->>'name') || ' ' || (params_json->>'error') FROM notify_problem WHERE key = '{DEAD_LETTER}'"
        ))
        .await;
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].starts_with("warning webhook.delivery failing webhook.error.status: 500"),
        "{problems:?}"
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        listener.received("failing").len(),
        usize::try_from(MAX_ATTEMPTS).expect("a count"),
        "a dead delivery is not attempted again"
    );

    listener.answer("failing", Answer::Status(204));
    let delivery = row["id"].as_str().expect("an id").to_owned();
    let retried = harness
        .call(Who::Admin, DELIVERY_RETRY, json!({ "id": delivery }))
        .await;
    assert_eq!(data(&retried)["state"], json!(STATE_PENDING));
    assert_eq!(data(&retried)["attempt"], json!(MAX_ATTEMPTS));
    wait_until("the retry delivers", || async {
        delivery_of(&harness, &subscription)
            .await
            .is_some_and(|row| row["state"] == json!(STATE_DELIVERED))
    })
    .await;
    let row = delivery_of(&harness, &subscription).await.expect("row");
    assert_eq!(row["attempt"], json!(MAX_ATTEMPTS + 1));
    assert_eq!(row["lastStatus"], json!(204));
    assert_eq!(row["lastError"], Value::Null);
    wait_until("the dead letter clears", || async {
        open_problems(&harness).await.is_empty()
    })
    .await;
    let again = harness
        .call(Who::Admin, DELIVERY_RETRY, json!({ "id": delivery }))
        .await;
    assert_eq!(again.status(), 409, "{:?}", again.envelope);
    assert_eq!(again.error().expect("an error").key, ERROR_NOT_RETRYABLE);
    harness.finish().await;
}

#[tokio::test]
async fn the_consumer_resumes_from_its_cursor_after_a_restart() {
    let harness = boot().await;
    let listener = Listener::start().await;
    let created = subscribe(
        &harness,
        Who::Admin,
        "survivor",
        &listener.url("survivor"),
        &[MASK_NOTES],
    )
    .await;
    let subscription = created["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    wait_until("the first delivery", || async {
        delivery_of(&harness, &subscription)
            .await
            .is_some_and(|row| row["state"] == json!(STATE_DELIVERED))
    })
    .await;
    let cursor = harness
        .database()
        .rows(&format!(
            "SELECT seq::text FROM webhook_cursor WHERE consumer = '{CURSOR}'"
        ))
        .await;
    let newest = harness
        .database()
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    assert_eq!(cursor, newest);

    let key = harness.key().clone();
    let database = harness.stop().await;
    let again = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .clock(at(START))
        .database(database)
        .key(key)
        .boot()
        .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        deliveries(&again).await.len(),
        1,
        "no duplicate delivery after the restart"
    );
    assert_eq!(listener.received("survivor").len(), 1);
    save(&again, Who::Admin).await;
    wait_until("the delivery after the restart", || async {
        listener.received("survivor").len() >= 2
    })
    .await;
    wait_until("both deliveries are delivered", || async {
        delivery_rows(&again, Who::Admin, json!({}))
            .await
            .iter()
            .all(|row| row["state"] == json!(STATE_DELIVERED))
            && deliveries(&again).await.len() == 2
    })
    .await;
    let cursor = again
        .database()
        .rows(&format!(
            "SELECT seq::text FROM webhook_cursor WHERE consumer = '{CURSOR}'"
        ))
        .await;
    let newest = again
        .database()
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    assert_eq!(cursor, newest);
    again.finish().await;
}

#[tokio::test]
async fn a_private_or_plain_url_is_refused_on_url_unless_webhook_private_targets_is_set() {
    let harness = boot().await;
    allow_private(&harness, false);
    answer(&harness, "private.test", &["10.0.0.5"]);
    answer(&harness, "mixed.test", &[PUBLIC, "192.168.1.9"]);
    answer(&harness, "public.test", &[PUBLIC]);
    for (url, reason) in [
        ("http://hooks.example/in", "scheme"),
        ("https://127.0.0.1/in", "ip_literal"),
        ("https://[::1]/in", "ip_literal"),
        ("https://8.8.8.8/in", "ip_literal"),
        ("https://private.test/in", "private_address"),
        ("https://mixed.test/in", "private_address"),
    ] {
        for who in [Who::A1, Who::Admin] {
            let reply = harness
                .call(
                    who,
                    SUBSCRIPTION_SET,
                    json!({ "name": "refused", "url": url, "events": [MASK_NOTES] }),
                )
                .await;
            let params = refused_on(&reply, "url", ERROR_URL_PRIVATE);
            assert_eq!(params["reason"], json!(reason), "{who} {url}");
        }
    }
    let accepted = subscribe(
        &harness,
        Who::A1,
        "public",
        "https://public.test/in",
        &[MASK_NOTES],
    )
    .await;
    assert_eq!(accepted["url"], json!("https://public.test/in"));
    let broken = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "broken", "url": "ftp://hooks.example/in", "events": [MASK_NOTES] }),
        )
        .await;
    assert_eq!(broken.status(), 422, "{:?}", broken.envelope);
    let unknown = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "unknown", "url": "https://nowhere.test/in", "events": [MASK_NOTES] }),
        )
        .await;
    refused_on(&unknown, "url", "webhook.error.url_unresolved");
    assert!(
        harness
            .call(Who::A1, SUBSCRIPTION_LIST, json!({}))
            .await
            .data()
            .is_some_and(|page| page["total"] == json!(1))
    );

    allow_private(&harness, true);
    answer(&harness, "hooks.example", &[PUBLIC]);
    for url in [
        "http://hooks.example/in",
        "https://127.0.0.1/in",
        "http://[::1]:9/in",
        "https://private.test/in",
    ] {
        let reply = harness
            .call(
                Who::A1,
                SUBSCRIPTION_SET,
                json!({ "name": format!("allowed {url}"), "url": url, "events": [MASK_NOTES] }),
            )
            .await;
        assert_eq!(data(&reply)["url"], json!(url));
    }
    let reply = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "still not a web address", "url": "ftp://hooks.example/in", "events": [MASK_NOTES] }),
        )
        .await;
    refused_on(&reply, "url", "webhook.error.url");
    harness.finish().await;
}

#[tokio::test]
async fn the_sender_connects_to_the_address_it_checked_and_not_to_one_the_name_answers_later() {
    let harness = boot().await;
    let listener = Listener::start().await;
    answer(&harness, "pinned.test", &["127.0.0.1"]);
    let pinned = subscribe(
        &harness,
        Who::Admin,
        "pinned",
        &format!("http://pinned.test:{}/hook/pinned", listener.port),
        &[MASK_NOTES],
    )
    .await;
    let pinned_id = pinned["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    wait_until("the pinned name reaches the listener", || async {
        !listener.received("pinned").is_empty()
    })
    .await;
    let hit = &listener.received("pinned")[0];
    assert_eq!(
        hit.header("host"),
        format!("pinned.test:{}", listener.port),
        "the request names the host while the connection went to the pinned address"
    );
    wait_until("the pinned delivery is marked delivered", || async {
        delivery_of(&harness, &pinned_id)
            .await
            .is_some_and(|row| row["state"] == json!(STATE_DELIVERED))
    })
    .await;
    let disabled = harness
        .call(
            Who::Admin,
            SUBSCRIPTION_TOGGLE,
            json!({ "id": pinned_id, "enabled": false }),
        )
        .await;
    assert_eq!(data(&disabled)["enabled"], json!(false));

    allow_private(&harness, false);
    answer(&harness, "rebinding.test", &[PUBLIC]);
    answer(&harness, "rebinding.test", &["127.0.0.1"]);
    let rebinding = subscribe(
        &harness,
        Who::Admin,
        "rebinding",
        "https://rebinding.test/hook/rebound",
        &[MASK_NOTES],
    )
    .await;
    let rebinding_id = rebinding["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    let row = wait_attempt(&harness, &rebinding_id, 1).await;
    assert_eq!(row["state"], json!(STATE_FAILED), "{row}");
    assert_eq!(row["lastStatus"], Value::Null);
    assert_eq!(
        row["lastError"],
        json!("webhook.error.url_private: private_address"),
        "{row}"
    );
    assert!(listener.received("rebound").is_empty());
    assert_eq!(listener.received("pinned").len(), 1);
    harness.finish().await;
}

#[tokio::test]
async fn a_short_secret_is_refused_on_secret_and_a_generated_one_is_32_bytes() {
    let harness = boot().await;
    let short = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "short", "url": "http://127.0.0.1:9/hook/x", "secret": "fifteen-chars-x", "events": [MASK_NOTES] }),
        )
        .await;
    let params = refused_on(&short, "secret", ERROR_SECRET_SHORT);
    assert_eq!(params["min"], json!(SECRET_MIN_BYTES));
    assert_eq!(SECRET_MIN_BYTES, 16);
    let generated = subscribe(
        &harness,
        Who::A1,
        "generated",
        "http://127.0.0.1:9/hook/x",
        &[MASK_NOTES],
    )
    .await;
    let secret = generated["secret"].as_str().expect("a generated secret");
    assert_eq!(SECRET_GENERATED_BYTES, 32);
    assert_eq!(
        secret.len(),
        base64url(&[7u8; SECRET_GENERATED_BYTES]).len(),
        "32 bytes are 43 base64url characters"
    );
    assert_eq!(secret.len(), 43);
    assert!(
        secret
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
        "{secret}"
    );
    let id = generated["id"].as_str().expect("an id");
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel provides the cipher");
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT secret FROM webhook_subscription WHERE id = '{id}'"
        ))
        .await;
    assert_ne!(stored[0], secret);
    assert_eq!(
        cipher.open(&stored[0]).expect("the data key opens it"),
        secret.as_bytes()
    );
    let chosen = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "id": id, "name": "chosen", "url": "http://127.0.0.1:9/hook/x", "secret": "sixteen-chars-xx", "events": [MASK_NOTES] }),
        )
        .await;
    assert_eq!(data(&chosen)["secret"], json!("sixteen-chars-xx"));
    let kept = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "id": id, "name": "kept", "url": "http://127.0.0.1:9/hook/x", "events": ["probe.other.*"] }),
        )
        .await;
    assert_eq!(data(&kept)["secret"], Value::Null);
    assert_eq!(data(&kept)["events"], json!(["probe.other.*"]));
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT secret FROM webhook_subscription WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(
        cipher.open(&stored[0]).expect("the data key opens it"),
        b"sixteen-chars-xx"
    );
    let got = harness
        .call(Who::A1, SUBSCRIPTION_GET, json!({ "id": id }))
        .await;
    assert!(data(&got).get("secret").is_none(), "{:?}", got.data());
    assert_eq!(data(&got)["name"], json!("kept"));
    let bad_mask = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "masks", "url": "http://127.0.0.1:9/hook/x", "events": ["probe.*", "Nope"] }),
        )
        .await;
    let params = refused_on(&bad_mask, "events[1]", ERROR_MASK);
    assert_eq!(params["mask"], json!("Nope"));
    let no_mask = harness
        .call(
            Who::A1,
            SUBSCRIPTION_SET,
            json!({ "name": "masks", "url": "http://127.0.0.1:9/hook/x", "events": [] }),
        )
        .await;
    assert_eq!(no_mask.status(), 422, "{:?}", no_mask.envelope);
    let deleted = harness
        .call(Who::A1, SUBSCRIPTION_DELETE, json!({ "id": id }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(true));
    let gone = harness
        .call(Who::A1, SUBSCRIPTION_DELETE, json!({ "id": id }))
        .await;
    assert_eq!(gone.status(), 404, "{:?}", gone.envelope);
    harness.finish().await;
}

#[tokio::test]
async fn the_sender_follows_no_redirect() {
    let harness = boot().await;
    let listener = Listener::start().await;
    let second = Listener::start().await;
    listener.answer("redirect", Answer::Redirect(second.url("second")));
    let created = subscribe(
        &harness,
        Who::Admin,
        "redirecting",
        &listener.url("redirect"),
        &[MASK_NOTES],
    )
    .await;
    let subscription = created["id"].as_str().expect("an id").to_owned();
    save(&harness, Who::Admin).await;
    let row = wait_attempt(&harness, &subscription, 1).await;
    assert_eq!(row["state"], json!(STATE_FAILED), "{row}");
    assert_eq!(row["lastStatus"], json!(302), "{row}");
    assert!(
        row["lastError"]
            .as_str()
            .is_some_and(|error| error.starts_with("webhook.error.status: 302")),
        "{row}"
    );
    tokio::time::sleep(QUIET).await;
    assert_eq!(listener.received("redirect").len(), 1);
    assert!(second.received("second").is_empty());
    harness.finish().await;
}

#[tokio::test]
async fn deleting_an_account_drops_its_subscriptions_and_deliveries() {
    let harness = boot().await;
    let listener = Listener::start().await;
    let a1 = harness.fixtures().accounts.a1;
    let a2 = harness.fixtures().accounts.a2;
    let fixture = SubscriptionFixture;
    let first = fixture.create(&harness, a1).await;
    let second = fixture.create(&harness, a1).await;
    let other = fixture.create(&harness, a2).await;
    for id in [&first, &second, &other] {
        let moved = harness
            .call(
                Who::Owner,
                SUBSCRIPTION_SET,
                json!({ "id": id, "name": format!("moved {id}"), "url": listener.url("drop"), "events": [MASK_NOTES] }),
            )
            .await;
        data(&moved);
    }
    save(&harness, Who::A1).await;
    save(&harness, Who::A2).await;
    wait_until("the deliveries are created", || async {
        deliveries(&harness).await.len() >= 3
    })
    .await;
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        deliveries(&harness).await.len(),
        3,
        "a subscription the owner placed in an account receives that account's events alone"
    );
    let db = Db::from_connection(harness.db().clone());
    let dropped = drop_account(&db, a1).await.expect("the rows drop");
    assert_eq!(dropped.subscriptions, 2);
    assert_eq!(dropped.deliveries, 2);
    let rows = deliveries(&harness).await;
    assert_eq!(rows.len(), 1);
    assert!(rows.iter().all(|row| row.starts_with(&other)), "{rows:?}");
    let left = harness
        .database()
        .rows("SELECT id FROM webhook_subscription ORDER BY id")
        .await;
    assert_eq!(left, std::slice::from_ref(&other));
    let again = drop_account(&db, a1)
        .await
        .expect("dropping twice is a no-op");
    assert_eq!(again.subscriptions, 0);
    assert_eq!(again.deliveries, 0);
    harness.finish().await;
}

#[tokio::test]
async fn the_standard_invariants_hold_for_subscriptions() {
    let harness = boot().await;
    let fixture = SubscriptionFixture;
    harness
        .assert_reseller_sees_only_subtree(SUBSCRIPTION_LIST, &fixture)
        .await;
    for function in [
        SUBSCRIPTION_GET,
        SUBSCRIPTION_SET,
        SUBSCRIPTION_TOGGLE,
        SUBSCRIPTION_DELETE,
    ] {
        harness
            .assert_user_cannot_edit_foreign_row(function, &fixture)
            .await;
    }
    let guest = harness.call(Who::Guest, SUBSCRIPTION_LIST, json!({})).await;
    assert_eq!(guest.status(), 401, "{:?}", guest.error());
    harness.finish().await;
}

#[tokio::test]
async fn the_webhook_descriptors_match_their_snapshots() {
    let harness = boot().await;
    assert_eq!(
        harness.modules(),
        ["keel", "cron", "notify", "testkit", "webhook", "probe"]
    );
    let http = harness.http().await;
    let answer = http.get(Who::A1, "/api/v1/boot").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let boot = answer.json();
    http.stop().await.expect("the server stops");
    let node = boot["menu"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["path"] == json!(MENU_PATH))
        .unwrap_or_else(|| panic!("no menu node {MENU_PATH} in {}", boot["menu"]));
    assert_eq!(node["func"], json!(SUBSCRIPTION_LIST));
    for level in Level::ALL {
        let names = harness.snapshot_descriptors_of(level, snapshots_dir(), MODULE.id);
        let webhook: Vec<&str> = names.iter().map(String::as_str).collect();
        if level < Level::User {
            assert!(webhook.is_empty(), "{level}: {webhook:?}");
        } else {
            let mut expected: Vec<&str> = FUNCTIONS.to_vec();
            expected.sort_unstable();
            assert_eq!(webhook, expected, "{level}");
        }
    }
    harness.finish().await;
}
