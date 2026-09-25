use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use keel_sdk::db::Db;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, CallerOptions, Frame, PrincipalSource};
use keel_sdk::types::{Envelope, NotificationFrame, NotificationLevel, Via};
use keel_system::cron::api::{Outcome, Scheduler};
use keel_system::notify::api::{
    EVENT_MESSAGE_CREATED, EVENT_PROBLEM_CLEARED, EVENT_PROBLEM_RAISED, FRAME_KIND,
    KEY_PROBLEM_CLEARED, KEY_PROBLEM_RAISED, Notifier, Problem, ProblemSource, Problems, Raise,
    Recipients, SOURCE_PROBLEM, Severity,
};
use keel_system::notify::{
    FUNCTIONS, INTERNAL_FUNCTIONS, MENU_PATH, MESSAGE_CLEAR, MESSAGE_CREATE, MESSAGE_LIST,
    MESSAGE_MARK, MODULE, POLL_JOB, PROBLEM_GET, PROBLEM_LIST, WIDGET_LIMIT, drop_account,
};
use keel_testkit::{EventStream, Fixture, Harness, Reply, Who, tokio};

const PATIENCE: Duration = Duration::from_secs(15);
const QUIET: Duration = Duration::from_millis(500);
const SOURCE: &str = "probe.disk";

static REPORTED: Mutex<Option<HashMap<String, Vec<Raise>>>> = Mutex::new(None);
static BROKEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn report(harness: &Harness, raises: Vec<Raise>) {
    REPORTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(harness.database().name().to_owned(), raises);
}

fn breaking(harness: &Harness, broken: bool) {
    let mut set = BROKEN.lock().unwrap_or_else(PoisonError::into_inner);
    let set = set.get_or_insert_with(HashSet::new);
    if broken {
        set.insert(harness.database().name().to_owned());
    } else {
        set.remove(harness.database().name());
    }
}

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

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["notify"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(ProblemSource::new(SOURCE, |db| async move {
        let name = database_name(&db).await?;
        if BROKEN
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|set| set.contains(&name))
        {
            return Err(KeelError::unavailable("probe.error.unreachable"));
        }
        Ok(REPORTED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|map| map.get(&name).cloned())
            .unwrap_or_default())
    }))
    .function(
        function::<Nothing, Nothing>("probe.notify.fail")
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let admin = call.principal().id;
                call.invoke(
                    MESSAGE_CREATE,
                    json!({
                        "recipients": [{ "principal": admin }],
                        "key": "probe.rolled_back",
                        "params": {},
                        "level": "info",
                        "source": "probe"
                    }),
                )
                .await?;
                Err(KeelError::conflict("probe.error.after_the_message"))
            }),
    );
    Ok(())
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

async fn boot() -> Harness {
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&PROBE)
        .boot()
        .await
}

fn caller(harness: &Harness) -> Arc<dyn Caller> {
    harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller")
}

fn scheduler(harness: &Harness) -> Arc<Scheduler> {
    harness
        .registry()
        .services()
        .get::<Scheduler>()
        .expect("the cron module provides the scheduler")
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn refused(reply: &Reply, status: u16, key: &str) {
    let error = reply
        .error()
        .unwrap_or_else(|| panic!("an error was expected, got {:?}", reply.envelope));
    assert_eq!(reply.status(), status, "{error:?}");
    assert_eq!(error.key, key, "{error:?}");
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn raise(harness: &Harness, raise: Raise) -> Problem {
    Problems::raise(caller(harness).as_ref(), raise)
        .await
        .expect("the problem is raised")
}

async fn events(harness: &Harness, kind: &str) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT data_json::text FROM keel_outbox WHERE type = '{kind}' ORDER BY seq"
        ))
        .await
}

async fn messages(harness: &Harness, who: Who) -> Vec<String> {
    harness
        .database()
        .rows(&format!(
            "SELECT key || ' ' || level || ' ' || source || ' ' || coalesce(problem_id, '-') FROM notify_message WHERE principal_id = '{}' ORDER BY created_at, id",
            harness.principal(who).id
        ))
        .await
}

async fn open_problems(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT key || ' ' || subject || ' ' || count::text FROM notify_problem WHERE cleared_at IS NULL ORDER BY key, subject")
        .await
}

async fn boot_of(harness: &Harness, who: Who) -> Value {
    let http = harness.http().await;
    let answer = http.get(who, "/api/v1/boot").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let boot = answer.json();
    http.stop().await.expect("the server stops");
    boot
}

async fn run_poll(harness: &Harness) {
    let scheduler = scheduler(harness);
    wait_until("the leader takes the lock", || {
        let scheduler = scheduler.clone();
        async move { scheduler.is_leader() }
    })
    .await;
    let started = scheduler.run_now(POLL_JOB).await.expect("the poll starts");
    let outcome = tokio::time::timeout(PATIENCE, scheduler.wait(started.run))
        .await
        .expect("the poll ends in time")
        .expect("the outcome is read");
    assert_eq!(outcome, Outcome::Ok);
}

struct ProblemFixture;

#[async_trait]
impl Fixture for ProblemFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        raise(
            harness,
            Raise::new("probe.quota", &account.to_string(), Severity::Warning).account(account),
        )
        .await
        .id
        .to_string()
    }
}

struct MessageFixture;

#[async_trait]
impl Fixture for MessageFixture {
    async fn create(&self, harness: &Harness, account: Id) -> String {
        let ids = Notifier::notify(
            caller(harness).as_ref(),
            Recipients::Account(account),
            "probe.hello",
            json!({}),
            NotificationLevel::Info,
        )
        .await
        .expect("the message is created");
        assert_eq!(ids.len(), 1, "one principal per fixture account");
        ids[0].to_string()
    }
}

#[tokio::test]
async fn raise_is_idempotent_by_key_and_subject_and_clear_closes_the_problem_once() {
    let harness = boot().await;
    let first = raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Critical)
            .params(json!({ "free": 5 }))
            .expect("params")
            .source(SOURCE),
    )
    .await;
    assert_eq!(first.count, 1);
    assert!(first.open);
    assert_eq!(first.params, json!({ "free": 5 }));
    assert_eq!(first.severity, Severity::Critical);
    assert_eq!(first.source, SOURCE);
    assert!(first.account_id.is_none());
    let again = raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Critical)
            .params(json!({ "free": 3 }))
            .expect("params")
            .source(SOURCE),
    )
    .await;
    assert_eq!(again.id, first.id);
    assert_eq!(again.count, 2);
    assert_eq!(again.params, json!({ "free": 3 }));
    assert_eq!(again.raised_at, first.raised_at);
    assert_eq!(open_problems(&harness).await, ["probe.disk / 2"]);
    assert_eq!(events(&harness, EVENT_PROBLEM_RAISED).await.len(), 1);
    assert_eq!(messages(&harness, Who::Admin).await.len(), 1);
    let other = raise(
        &harness,
        Raise::new("probe.disk", "/var", Severity::Warning).source(SOURCE),
    )
    .await;
    assert_ne!(other.id, first.id);
    assert_eq!(open_problems(&harness).await.len(), 2);

    let cleared = Problems::clear(caller(&harness).as_ref(), "probe.disk", "/")
        .await
        .expect("the clear runs");
    assert!(cleared.cleared);
    let problem = cleared.problem.expect("the cleared problem");
    assert_eq!(problem.id, first.id);
    assert!(!problem.open);
    assert!(problem.cleared_at.is_some());
    let twice = Problems::clear(caller(&harness).as_ref(), "probe.disk", "/")
        .await
        .expect("a second clear is a no-op");
    assert!(!twice.cleared);
    assert!(twice.problem.is_none());
    let unknown = Problems::clear(caller(&harness).as_ref(), "probe.nothing", "/")
        .await
        .expect("clearing a pair that never opened is a no-op");
    assert!(!unknown.cleared);
    assert_eq!(open_problems(&harness).await, ["probe.disk /var 1"]);
    assert_eq!(events(&harness, EVENT_PROBLEM_CLEARED).await.len(), 1);
    let admin = messages(&harness, Who::Admin).await;
    assert_eq!(admin.len(), 3);
    assert!(admin[2].starts_with(&format!("{KEY_PROBLEM_CLEARED} info {SOURCE_PROBLEM} ")));

    let reopened = raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Critical).source(SOURCE),
    )
    .await;
    assert_ne!(reopened.id, first.id);
    assert_eq!(reopened.count, 1);
    let got = harness
        .call(Who::Admin, PROBLEM_GET, json!({ "id": first.id }))
        .await;
    assert_eq!(data(&got)["open"], json!(false));
    assert_eq!(data(&got)["count"], json!(2));
    let listed = harness
        .call(
            Who::Admin,
            PROBLEM_LIST,
            json!({ "filters": [{ "key": "open", "op": "eq", "value": "true" }] }),
        )
        .await;
    assert_eq!(data(&listed)["total"], json!(2));
    let by_severity = harness
        .call(
            Who::Admin,
            PROBLEM_LIST,
            json!({ "filters": [{ "key": "severity", "op": "eq", "value": "critical" }], "sort": { "key": "raisedAt", "dir": "asc" } }),
        )
        .await;
    let rows = data(&by_severity)["data"].as_array().expect("rows").clone();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], json!(first.id));
    assert_eq!(rows[1]["id"], json!(reopened.id));
    harness.finish().await;
}

#[tokio::test]
async fn a_source_polled_by_the_cron_job_raises_what_it_reports_and_clears_what_it_stops_reporting()
{
    let harness = boot().await;
    let a1 = harness.fixtures().accounts.a1;
    report(
        &harness,
        vec![
            Raise::new("probe.disk", "/", Severity::Warning),
            Raise::new("probe.quota", &a1.to_string(), Severity::Warning).account(a1),
        ],
    );
    run_poll(&harness).await;
    assert_eq!(
        open_problems(&harness).await,
        ["probe.disk / 1".to_owned(), format!("probe.quota {a1} 1")]
    );
    let sources = harness
        .database()
        .rows("SELECT DISTINCT source FROM notify_problem")
        .await;
    assert_eq!(sources, [SOURCE]);
    assert_eq!(messages(&harness, Who::A1).await.len(), 1);
    assert_eq!(messages(&harness, Who::A2).await.len(), 0);

    report(
        &harness,
        vec![Raise::new("probe.disk", "/", Severity::Warning)],
    );
    run_poll(&harness).await;
    assert_eq!(open_problems(&harness).await, ["probe.disk / 2"]);
    assert_eq!(messages(&harness, Who::A1).await.len(), 2);
    assert!(
        messages(&harness, Who::A1).await[1].starts_with(&format!("{KEY_PROBLEM_CLEARED} info "))
    );

    breaking(&harness, true);
    run_poll(&harness).await;
    assert_eq!(open_problems(&harness).await, ["probe.disk / 2"]);
    breaking(&harness, false);

    report(&harness, Vec::new());
    run_poll(&harness).await;
    assert!(open_problems(&harness).await.is_empty());
    assert_eq!(events(&harness, EVENT_PROBLEM_RAISED).await.len(), 2);
    assert_eq!(events(&harness, EVENT_PROBLEM_CLEARED).await.len(), 2);
    let listed = harness
        .call(
            Who::Admin,
            keel_system::cron::JOB_LIST,
            json!({ "filters": [{ "key": "id", "op": "eq", "value": POLL_JOB }] }),
        )
        .await;
    assert_eq!(data(&listed)["data"][0]["schedule"], json!("* * * * *"));
    assert_eq!(data(&listed)["data"][0]["lastOutcome"], json!("ok"));
    harness.finish().await;
}

#[tokio::test]
async fn a_raised_problem_notifies_the_admins_once_counts_in_boot_and_feeds_the_dashboard_widget() {
    let harness = boot().await;
    let before = boot_of(&harness, Who::Admin).await;
    assert_eq!(before["problems"], json!({ "open": 0, "critical": 0 }));
    raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Warning).source(SOURCE),
    )
    .await;
    raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Warning).source(SOURCE),
    )
    .await;
    let admin = boot_of(&harness, Who::Admin).await;
    assert_eq!(admin["problems"], json!({ "open": 1, "critical": 0 }));
    assert_eq!(
        admin["dashboard"],
        json!([
            { "kind": "stat", "func": "keel.widget.accounts" },
            { "kind": "list", "func": PROBLEM_LIST, "limit": WIDGET_LIMIT }
        ])
    );
    let paths: Vec<&str> = admin["menu"]
        .as_array()
        .expect("groups")
        .iter()
        .flat_map(|group| group["items"].as_array().into_iter().flatten())
        .filter_map(|item| item["path"].as_str())
        .collect();
    assert!(paths.contains(&MENU_PATH), "{paths:?}");
    let user = boot_of(&harness, Who::A1).await;
    assert_eq!(user["problems"], json!({ "open": 0, "critical": 0 }));
    assert_eq!(user["dashboard"], json!([]));
    let owner = boot_of(&harness, Who::Owner).await;
    assert_eq!(owner["problems"], json!({ "open": 1, "critical": 0 }));
    assert_eq!(owner["dashboard"], admin["dashboard"]);
    let guest = boot_of(&harness, Who::Guest).await;
    assert_eq!(guest["problems"], json!({ "open": 0, "critical": 0 }));
    let created = events(&harness, EVENT_MESSAGE_CREATED).await;
    assert_eq!(created.len(), 1, "{created:?}");
    let event: Value = serde_json::from_str(&created[0]).expect("event data");
    assert_eq!(event["principal"], json!(harness.principal(Who::Admin).id));
    assert_eq!(event["key"], json!(KEY_PROBLEM_RAISED));
    assert_eq!(event["level"], json!("warning"));
    assert_eq!(event["source"], json!(SOURCE_PROBLEM));
    let admin_messages = messages(&harness, Who::Admin).await;
    assert_eq!(admin_messages.len(), 1);
    assert!(
        admin_messages[0].starts_with(&format!("{KEY_PROBLEM_RAISED} warning {SOURCE_PROBLEM} "))
    );
    assert!(messages(&harness, Who::A1).await.is_empty());
    assert!(messages(&harness, Who::Reseller).await.is_empty());
    let listed = harness.call(Who::Admin, MESSAGE_LIST, json!({})).await;
    let row = &data(&listed)["data"][0];
    assert_eq!(row["key"], json!(KEY_PROBLEM_RAISED));
    assert_eq!(row["params"]["key"], json!("probe.disk"));
    assert_eq!(row["params"]["count"], json!(1));
    assert_eq!(row["unread"], json!(true));
    harness
        .clock()
        .advance(keel_sdk::chrono::Duration::seconds(60));
    let critical = raise(
        &harness,
        Raise::new("probe.disk", "/var", Severity::Critical).source(SOURCE),
    )
    .await;
    assert_eq!(critical.severity, Severity::Critical);
    assert_eq!(
        boot_of(&harness, Who::Admin).await["problems"],
        json!({ "open": 2, "critical": 1 })
    );
    let widget = harness
        .call(
            Who::Admin,
            PROBLEM_LIST,
            json!({ "pageSize": WIDGET_LIMIT }),
        )
        .await;
    assert_eq!(data(&widget)["total"], json!(2));
    assert_eq!(data(&widget)["data"][0]["id"], json!(critical.id));
    harness.finish().await;
}

#[tokio::test]
async fn an_account_scoped_problem_reaches_the_accounts_principals_and_shows_in_their_boot_counts()
{
    let harness = boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let problem = raise(
        &harness,
        Raise::new("probe.quota", &a1.to_string(), Severity::Warning)
            .account(a1)
            .source(SOURCE),
    )
    .await;
    assert_eq!(problem.account_id, Some(a1));
    assert_eq!(messages(&harness, Who::A1).await.len(), 1);
    assert_eq!(messages(&harness, Who::Admin).await.len(), 1);
    assert!(messages(&harness, Who::A2).await.is_empty());
    assert!(messages(&harness, Who::Reseller).await.is_empty());
    assert!(messages(&harness, Who::Unrelated).await.is_empty());
    for (who, open) in [
        (Who::A1, 1),
        (Who::A2, 0),
        (Who::Reseller, 1),
        (Who::Unrelated, 0),
        (Who::Admin, 1),
        (Who::Owner, 1),
    ] {
        assert_eq!(
            boot_of(&harness, who).await["problems"],
            json!({ "open": open, "critical": 0 }),
            "{who}"
        );
    }
    let a1_list = harness.call(Who::A1, PROBLEM_LIST, json!({})).await;
    assert_eq!(data(&a1_list)["total"], json!(1));
    assert_eq!(data(&a1_list)["data"][0]["id"], json!(problem.id));
    assert!(data(&a1_list)["data"][0].get("accountId").is_none());
    let reseller_list = harness.call(Who::Reseller, PROBLEM_LIST, json!({})).await;
    assert_eq!(data(&reseller_list)["data"][0]["accountId"], json!(a1));
    let a2_list = harness.call(Who::A2, PROBLEM_LIST, json!({})).await;
    assert_eq!(data(&a2_list)["total"], json!(0));
    let a1_get = harness
        .call(Who::A1, PROBLEM_GET, json!({ "id": problem.id }))
        .await;
    assert_eq!(data(&a1_get)["subject"], json!(a1.to_string()));
    let a2_get = harness
        .call(Who::A2, PROBLEM_GET, json!({ "id": problem.id }))
        .await;
    refused(&a2_get, 404, "keel.error.not_found");
    let global = raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Warning).source(SOURCE),
    )
    .await;
    let hidden = harness
        .call(Who::A1, PROBLEM_GET, json!({ "id": global.id }))
        .await;
    refused(&hidden, 404, "keel.error.not_found");
    let reseller_hidden = harness
        .call(Who::Reseller, PROBLEM_GET, json!({ "id": global.id }))
        .await;
    refused(&reseller_hidden, 404, "keel.error.not_found");
    let admin_get = harness
        .call(Who::Admin, PROBLEM_GET, json!({ "id": global.id }))
        .await;
    assert_eq!(data(&admin_get)["id"], json!(global.id));
    assert_eq!(
        boot_of(&harness, Who::Reseller).await["problems"],
        json!({ "open": 1, "critical": 0 })
    );
    assert_eq!(
        boot_of(&harness, Who::Admin).await["problems"],
        json!({ "open": 2, "critical": 0 })
    );
    let missing = harness
        .call(Who::Admin, PROBLEM_GET, json!({ "id": Id::generate() }))
        .await;
    refused(&missing, 404, "keel.error.not_found");
    harness.finish().await;
}

#[tokio::test]
async fn a_notification_frame_is_published_after_commit_and_not_for_a_rolled_back_message() {
    let harness = boot().await;
    let mut frames = harness.live_events();
    let admin = harness.principal(Who::Admin).clone();
    let failed = caller(&harness)
        .call(
            admin.clone(),
            "probe.notify.fail",
            json!({}),
            CallerOptions::default(),
        )
        .await
        .expect_err("the handler fails after creating the message");
    assert_eq!(failed.key(), "probe.error.after_the_message");
    tokio::time::sleep(QUIET).await;
    assert!(
        frames.try_recv().is_err(),
        "no frame for a rolled-back message"
    );
    assert!(messages(&harness, Who::Admin).await.is_empty());
    assert!(events(&harness, EVENT_MESSAGE_CREATED).await.is_empty());

    let ids = Notifier::notify(
        caller(&harness).as_ref(),
        Recipients::Principal(admin.id),
        "probe.hello",
        json!({ "who": "admin" }),
        NotificationLevel::Warning,
    )
    .await
    .expect("the message is created");
    assert_eq!(ids.len(), 1);
    let frame: Frame = tokio::time::timeout(PATIENCE, frames.recv())
        .await
        .expect("a frame arrives in time")
        .expect("the bus lives");
    assert_eq!(frame.kind, FRAME_KIND);
    assert_eq!(frame.principal, admin.id);
    assert_eq!(frame.account, admin.account_id);
    let notification: NotificationFrame =
        serde_json::from_value(frame.data.clone()).expect("a notification frame");
    assert_eq!(notification.id, ids[0]);
    assert_eq!(notification.key, "probe.hello");
    assert_eq!(notification.params, json!({ "who": "admin" }));
    assert_eq!(notification.level, NotificationLevel::Warning);
    assert_eq!(notification.created_at, harness.clock().now());
    assert_eq!(
        frame.data["createdAt"],
        json!(
            harness
                .clock()
                .now()
                .to_rfc3339_opts(keel_sdk::chrono::SecondsFormat::Millis, true)
        )
    );
    assert_eq!(
        messages(&harness, Who::Admin).await,
        ["probe.hello warning probe -"]
    );
    assert_eq!(events(&harness, EVENT_MESSAGE_CREATED).await.len(), 1);
    harness.finish().await;
}

async fn notification_on(stream: &mut EventStream) -> NotificationFrame {
    loop {
        let frame = stream.next_event().await;
        if frame.event.as_deref() != Some(FRAME_KIND) {
            continue;
        }
        assert_eq!(frame.id, None, "{frame:?}");
        return serde_json::from_value(frame.json()).expect("a notification frame");
    }
}

async fn no_notification_on(stream: &mut EventStream, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match stream.next_within(remaining).await {
            Ok(Some(frame)) if frame.event.as_deref() == Some(FRAME_KIND) => return false,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return true,
        }
    }
}

#[tokio::test]
async fn a_notification_frame_arrives_on_the_recipients_stream_when_a_message_is_written() {
    let harness = boot().await;
    let http = harness.http().await;
    let mut recipient = http.events(Who::Admin, None).await;
    let mut bystander = http.events(Who::A1, None).await;
    for stream in [&mut recipient, &mut bystander] {
        assert_eq!(stream.status(), 200);
        assert!(stream.next().await.is_comment());
    }
    let admin = harness.principal(Who::Admin).clone();
    let ids = Notifier::notify(
        caller(&harness).as_ref(),
        Recipients::Principal(admin.id),
        "probe.hello",
        json!({ "who": "admin" }),
        NotificationLevel::Warning,
    )
    .await
    .expect("the message is created");
    assert_eq!(ids.len(), 1);
    let notification = notification_on(&mut recipient).await;
    assert_eq!(notification.id, ids[0]);
    assert_eq!(notification.key, "probe.hello");
    assert_eq!(notification.params, json!({ "who": "admin" }));
    assert_eq!(notification.level, NotificationLevel::Warning);
    assert_eq!(notification.created_at, harness.clock().now());
    assert!(
        no_notification_on(&mut bystander, QUIET).await,
        "a user's stream carries no notification addressed to the admin"
    );
    assert_eq!(
        messages(&harness, Who::Admin).await,
        ["probe.hello warning probe -"]
    );
    drop((recipient, bystander));
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[tokio::test]
async fn notify_message_create_is_refused_over_http_and_the_socket_and_runs_through_the_caller() {
    let harness = boot().await;
    let admin = harness.principal(Who::Admin).clone();
    let body = json!({
        "recipients": ["admins"],
        "key": "probe.hello",
        "params": {},
        "level": "info",
        "source": "probe"
    });
    let http = harness.http().await;
    for func in INTERNAL_FUNCTIONS {
        let answer = http
            .post(Who::Owner, &format!("/api/v1/call/{func}"), &body)
            .await;
        assert_eq!(answer.status, 403, "{func}: {}", answer.body);
        let envelope = answer.json();
        assert_eq!(envelope["error"]["key"], json!("keel.error.internal_only"));
        assert_eq!(envelope["error"]["params"]["function"], json!(func));
        let described = http
            .get(Who::Owner, &format!("/api/v1/functions/{func}"))
            .await;
        assert_eq!(described.status, 403, "{func}: {}", described.body);
    }
    let index = http.get(Who::Owner, "/api/v1/functions").await;
    let names: Vec<String> = index
        .json()
        .as_array()
        .expect("descriptors")
        .iter()
        .filter_map(|descriptor| descriptor["name"].as_str())
        .filter(|name| name.starts_with("notify."))
        .map(str::to_owned)
        .collect();
    let mut expected: Vec<String> = FUNCTIONS.iter().map(|name| (*name).to_owned()).collect();
    expected.sort_unstable();
    assert_eq!(names, expected);
    http.stop().await.expect("the server stops");

    let socket = harness.socket().await;
    let over_socket = socket.call(MESSAGE_CREATE, body.clone()).await;
    let Envelope::Err { error } = over_socket else {
        panic!("the socket admitted an internal function: {over_socket:?}");
    };
    assert_eq!(error.key, "keel.error.internal_only");
    let as_admin = socket
        .call_as(MESSAGE_CREATE, body.clone(), Who::Admin.login())
        .await;
    assert!(
        matches!(as_admin, Envelope::Err { ref error } if error.key == "keel.error.internal_only"),
        "{as_admin:?}"
    );
    socket.stop().await.expect("the socket stops");
    let direct = harness.call(Who::Owner, MESSAGE_CREATE, body.clone()).await;
    refused(&direct, 403, "keel.error.internal_only");
    assert!(messages(&harness, Who::Admin).await.is_empty());

    let created = caller(&harness)
        .call(
            admin.clone(),
            MESSAGE_CREATE,
            body,
            CallerOptions::default(),
        )
        .await
        .expect("the caller reaches the internal function");
    assert_eq!(created["ids"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        messages(&harness, Who::Admin).await,
        ["probe.hello info probe -"]
    );
    let stored = harness
        .database()
        .rows("SELECT actor_json->>'id' FROM keel_outbox WHERE type = 'notify.message.created'")
        .await;
    assert_eq!(stored, [admin.id.to_string()]);
    assert_eq!(keel_system::cron::api::owner().via, Via::Internal);
    harness.finish().await;
}

#[tokio::test]
async fn a_user_lists_marks_and_clears_own_messages_only() {
    let harness = boot().await;
    let a1 = harness.principal(Who::A1).clone();
    let a2 = harness.principal(Who::A2).clone();
    let caller = caller(&harness);
    let mut ids = Vec::new();
    for (who, key) in [
        (&a1, "probe.first"),
        (&a1, "probe.second"),
        (&a2, "probe.other"),
    ] {
        harness
            .clock()
            .advance(keel_sdk::chrono::Duration::seconds(60));
        let created = Notifier::notify(
            caller.as_ref(),
            Recipients::Principal(who.id),
            key,
            json!({ "n": ids.len() }),
            NotificationLevel::Info,
        )
        .await
        .expect("the message is created");
        ids.extend(created);
    }
    let listed = harness.call(Who::A1, MESSAGE_LIST, json!({})).await;
    let page = data(&listed);
    assert_eq!(page["total"], json!(2));
    assert_eq!(page["data"][0]["key"], json!("probe.second"));
    assert_eq!(page["data"][1]["key"], json!("probe.first"));
    assert_eq!(page["data"][0]["unread"], json!(true));
    assert_eq!(page["data"][0]["readAt"], Value::Null);
    let other = harness.call(Who::A2, MESSAGE_LIST, json!({})).await;
    assert_eq!(data(&other)["total"], json!(1));
    assert_eq!(data(&other)["data"][0]["key"], json!("probe.other"));
    let admin = harness.call(Who::Admin, MESSAGE_LIST, json!({})).await;
    assert_eq!(data(&admin)["total"], json!(0));

    harness
        .clock()
        .advance(keel_sdk::chrono::Duration::seconds(60));
    let marked = harness
        .call(Who::A1, MESSAGE_MARK, json!({ "ids": [ids[0]] }))
        .await;
    assert_eq!(data(&marked)["marked"], json!(1));
    let unread = harness
        .call(
            Who::A1,
            MESSAGE_LIST,
            json!({ "filters": [{ "key": "unread", "op": "eq", "value": "true" }] }),
        )
        .await;
    assert_eq!(data(&unread)["total"], json!(1));
    assert_eq!(data(&unread)["data"][0]["id"], json!(ids[1]));
    let read = harness
        .call(
            Who::A1,
            MESSAGE_LIST,
            json!({ "filters": [{ "key": "unread", "op": "eq", "value": "false" }] }),
        )
        .await;
    assert_eq!(data(&read)["data"][0]["id"], json!(ids[0]));
    let read_at: keel_sdk::chrono::DateTime<keel_sdk::chrono::Utc> =
        data(&read)["data"][0]["readAt"]
            .as_str()
            .expect("a read time")
            .parse()
            .expect("an instant");
    assert_eq!(read_at, harness.clock().now());
    let unmarked = harness
        .call(
            Who::A1,
            MESSAGE_MARK,
            json!({ "ids": [ids[0]], "read": false }),
        )
        .await;
    assert_eq!(data(&unmarked)["marked"], json!(1));
    let all_unread = harness
        .call(
            Who::A1,
            MESSAGE_LIST,
            json!({ "filters": [{ "key": "unread", "op": "eq", "value": "true" }] }),
        )
        .await;
    assert_eq!(data(&all_unread)["total"], json!(2));
    harness
        .call(Who::A1, MESSAGE_MARK, json!({ "ids": [ids[0]] }))
        .await;

    let foreign = harness
        .call(Who::A1, MESSAGE_MARK, json!({ "ids": [ids[2]] }))
        .await;
    refused(&foreign, 404, "keel.error.not_found");
    let mixed = harness
        .call(Who::A1, MESSAGE_MARK, json!({ "ids": [ids[1], ids[2]] }))
        .await;
    refused(&mixed, 404, "keel.error.not_found");
    assert_eq!(messages(&harness, Who::A2).await.len(), 1);
    let owner_marks = harness
        .call(Who::Owner, MESSAGE_MARK, json!({ "ids": [ids[2]] }))
        .await;
    assert_eq!(data(&owner_marks)["marked"], json!(0));

    let cleared = harness.call(Who::A1, MESSAGE_CLEAR, json!({})).await;
    assert_eq!(data(&cleared)["deleted"], json!(1));
    let left = harness.call(Who::A1, MESSAGE_LIST, json!({})).await;
    assert_eq!(data(&left)["total"], json!(1));
    assert_eq!(data(&left)["data"][0]["id"], json!(ids[1]));
    assert_eq!(messages(&harness, Who::A2).await.len(), 1);
    let nothing = harness.call(Who::A2, MESSAGE_CLEAR, json!({})).await;
    assert_eq!(data(&nothing)["deleted"], json!(0));
    let guest = harness.call(Who::Guest, MESSAGE_LIST, json!({})).await;
    assert_eq!(guest.status(), 401, "{:?}", guest.error());

    harness
        .assert_reseller_sees_only_subtree(PROBLEM_LIST, &ProblemFixture)
        .await;
    harness
        .assert_user_cannot_edit_foreign_row(MESSAGE_MARK, &MessageFixture)
        .await;
    harness.finish().await;
}

#[tokio::test]
async fn deleting_an_account_drops_its_messages_and_problems() {
    let harness = boot().await;
    let a1 = harness.fixtures().accounts.a1;
    let a2 = harness.fixtures().accounts.a2;
    raise(
        &harness,
        Raise::new("probe.quota", &a1.to_string(), Severity::Warning)
            .account(a1)
            .source(SOURCE),
    )
    .await;
    raise(
        &harness,
        Raise::new("probe.quota", &a2.to_string(), Severity::Warning)
            .account(a2)
            .source(SOURCE),
    )
    .await;
    raise(
        &harness,
        Raise::new("probe.disk", "/", Severity::Warning).source(SOURCE),
    )
    .await;
    assert_eq!(messages(&harness, Who::A1).await.len(), 1);
    assert_eq!(messages(&harness, Who::A2).await.len(), 1);
    assert_eq!(messages(&harness, Who::Admin).await.len(), 3);
    assert_eq!(open_problems(&harness).await.len(), 3);

    let db = keel_sdk::Db::from_connection(harness.db().clone());
    let dropped = drop_account(&db, a1).await.expect("the rows drop");
    assert_eq!(dropped.messages, 2);
    assert_eq!(dropped.problems, 1);
    assert!(messages(&harness, Who::A1).await.is_empty());
    assert_eq!(messages(&harness, Who::A2).await.len(), 1);
    assert_eq!(messages(&harness, Who::Admin).await.len(), 2);
    assert_eq!(
        open_problems(&harness).await,
        ["probe.disk / 1".to_owned(), format!("probe.quota {a2} 1")]
    );
    let again = drop_account(&db, a1)
        .await
        .expect("dropping twice is a no-op");
    assert_eq!(again.messages, 0);
    assert_eq!(again.problems, 0);
    let source = harness
        .registry()
        .services()
        .get::<dyn PrincipalSource>()
        .expect("the kernel provides the principal source");
    assert_eq!(
        source
            .of_account(a1, &db)
            .await
            .expect("the principals stay")
            .len(),
        1
    );
    harness.finish().await;
}

#[tokio::test]
async fn the_notify_descriptors_match_their_snapshots_and_the_internal_functions_have_none() {
    let harness = boot().await;
    assert_eq!(
        harness.modules(),
        ["keel", "cron", "notify", "probe", "testkit"]
    );
    for level in Level::ALL {
        let names = harness.snapshot_descriptors_of(level, snapshots_dir(), MODULE.id);
        let notify: Vec<&str> = names.iter().map(String::as_str).collect();
        if level < Level::User {
            assert!(notify.is_empty(), "{level}: {notify:?}");
        } else {
            let mut expected: Vec<&str> = FUNCTIONS
                .iter()
                .copied()
                .filter(|name| {
                    level >= Level::Admin
                        || !(name.starts_with("notify.channel.")
                            || name.starts_with("notify.delivery."))
                })
                .collect();
            expected.sort_unstable();
            assert_eq!(notify, expected, "{level}");
        }
        for internal in INTERNAL_FUNCTIONS {
            assert!(
                !names.iter().any(|name| name == internal),
                "{level}: {internal}"
            );
            assert!(
                !snapshots_dir()
                    .join(format!("{internal}@{level}.snap"))
                    .exists(),
                "{internal} has a snapshot at {level}"
            );
        }
    }
    harness.finish().await;
}
