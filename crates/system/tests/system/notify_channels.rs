use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, SecretCipher};
use keel_system::notify::api::{
    KEY_PROBLEM_RAISED, KEY_TEST, MailTemplate, Problems, Raise, Severity,
};
use keel_system::notify::{
    CHANNEL_DELETE, CHANNEL_GET, CHANNEL_LIST, CHANNEL_SET, CHANNEL_TEST, CURSOR, DELIVERY_FAILED,
    DELIVERY_LIST, ERROR_KIND_UNAVAILABLE, ERROR_TEMPLATE_PLACEHOLDER, ERROR_TEMPLATE_TWICE,
    LADDER, MAX_ATTEMPTS, MODULE, STATE_FAILED, STATE_PENDING, STATE_SENT,
};
use keel_testkit::{Harness, Reply, Who, tokio};

const PATIENCE: Duration = Duration::from_secs(20);
const PROMPT: Duration = Duration::from_secs(2);
const START: &str = "2026-09-15T10:00:00Z";

static DOUBLED: ModuleDef = ModuleDef {
    id: "doubled",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["notify"],
    migrations: None,
    init: doubled_init,
};

fn doubled_init(r: &mut Registrar) -> Result<()> {
    r.contribute(MailTemplate::new(
        "doubled.hello",
        "Hello {{name}}",
        "Twice the braces",
    ));
    Ok(())
}

static COLLIDING: ModuleDef = ModuleDef {
    id: "colliding",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &["notify"],
    migrations: None,
    init: colliding_init,
};

fn colliding_init(r: &mut Registrar) -> Result<()> {
    r.contribute(MailTemplate::new(KEY_TEST, "Another test", "body"));
    Ok(())
}

fn snapshots_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().expect("an instant")
}

async fn boot() -> Harness {
    Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&keel_system::audit::MODULE)
        .clock(at(START))
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

async fn wait_until<F, Fut>(what: &str, mut check: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let started = Instant::now();
    while !check().await {
        assert!(
            started.elapsed() < PATIENCE,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    started.elapsed()
}

async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port is free");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}

fn email(port: u16, security: &str, password: Option<&str>) -> Value {
    json!({
        "host": "127.0.0.1",
        "port": port,
        "security": security,
        "username": "keel",
        "password": password,
        "from": "keel@example.org",
        "to": "ops@example.org"
    })
}

async fn set_channel(harness: &Harness, name: &str, enabled: bool, email: Value) -> Id {
    let reply = harness
        .call(
            Who::Admin,
            CHANNEL_SET,
            json!({ "kind": "email", "name": name, "enabled": enabled, "email": email }),
        )
        .await;
    data(&reply)["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a ulid")
}

async fn deliveries(harness: &Harness) -> Vec<String> {
    harness
        .database()
        .rows("SELECT channel || ' ' || message_key || ' ' || state || ' ' || attempt::text FROM notify_delivery ORDER BY channel, created_at, id")
        .await
}

async fn delivery_row(harness: &Harness, id: Id) -> Value {
    let listed = harness
        .call(
            Who::Admin,
            DELIVERY_LIST,
            json!({ "filters": [{ "key": "state", "op": "ne", "value": "nothing" }] }),
        )
        .await;
    data(&listed)["data"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == json!(id.to_string()))
        .cloned()
        .unwrap_or_else(|| panic!("no delivery {id} in {:?}", listed.data()))
}

async fn test_channel(harness: &Harness, channel: Id) -> (Id, Id) {
    let tested = harness
        .call(Who::Admin, CHANNEL_TEST, json!({ "id": channel }))
        .await;
    let message: Id = data(&tested)["message"]
        .as_str()
        .expect("a message id")
        .parse()
        .expect("a ulid");
    assert_eq!(
        data(&tested).as_object().map(|fields| fields.len()),
        Some(1)
    );
    wait_until(
        &format!("the delivery of the test message {message}"),
        || async { delivery_of_message(harness, message).await.is_some() },
    )
    .await;
    let delivery = delivery_of_message(harness, message)
        .await
        .expect("the delivery appeared");
    (message, delivery)
}

async fn delivery_of_message(harness: &Harness, message: Id) -> Option<Id> {
    let listed = harness
        .call(Who::Admin, DELIVERY_LIST, json!({ "search": KEY_TEST }))
        .await;
    data(&listed)["data"]
        .as_array()?
        .iter()
        .find(|row| row["messageId"] == json!(message.to_string()))?["id"]
        .as_str()?
        .parse()
        .ok()
}

async fn channel_row(harness: &Harness, id: Id) -> Reply {
    harness
        .call(Who::Admin, CHANNEL_GET, json!({ "id": id }))
        .await
}

async fn wait_attempt(harness: &Harness, id: Id, attempt: i64) -> Value {
    wait_until(&format!("attempt {attempt} of {id}"), || async {
        delivery_row(harness, id).await["attempt"].as_i64() >= Some(attempt)
    })
    .await;
    delivery_row(harness, id).await
}

#[tokio::test]
async fn an_unimplemented_channel_kind_is_refused_on_kind_and_the_email_password_is_sealed_and_masked()
 {
    let harness = boot().await;
    for kind in ["telegram", "webhook"] {
        let refusedreply = harness
            .call(
                Who::Admin,
                CHANNEL_SET,
                json!({ "kind": kind, "name": "chat", "enabled": true }),
            )
            .await;
        refused(&refusedreply, 422, "keel.error.validation");
        let error = refusedreply.error().expect("an error");
        assert_eq!(error.fields[0].path, "kind");
        assert_eq!(error.fields[0].key.as_deref(), Some(ERROR_KIND_UNAVAILABLE));
        assert_eq!(error.fields[0].params["kind"], json!(kind));
    }
    let without_email = harness
        .call(
            Who::Admin,
            CHANNEL_SET,
            json!({ "kind": "email", "name": "mail", "enabled": true }),
        )
        .await;
    refused(&without_email, 422, "keel.error.validation");
    assert_eq!(
        without_email.error().expect("an error").fields[0]
            .key
            .as_deref(),
        Some("notify.error.channel_email_missing")
    );
    let unknown = harness
        .call(
            Who::Admin,
            CHANNEL_SET,
            json!({ "kind": "pigeon", "name": "coop", "enabled": true }),
        )
        .await;
    assert_eq!(unknown.status(), 422, "{:?}", unknown.error());

    let id = set_channel(
        &harness,
        "ops mail",
        true,
        email(2525, "starttls", Some("hunter2")),
    )
    .await;
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT config_json->>'password' FROM notify_channel WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], "hunter2");
    assert!(!stored[0].contains("hunter2"), "{}", stored[0]);
    let cipher = harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel provides the cipher");
    assert_eq!(
        cipher.open(&stored[0]).expect("the data key opens it"),
        b"hunter2"
    );
    let got = harness
        .call(Who::Admin, CHANNEL_GET, json!({ "id": id }))
        .await;
    let channel = data(&got);
    assert_eq!(channel["kind"], json!("email"));
    assert_eq!(channel["name"], json!("ops mail"));
    assert_eq!(channel["enabled"], json!(true));
    assert_eq!(channel["email"]["host"], json!("127.0.0.1"));
    assert_eq!(channel["email"]["port"], json!(2525));
    assert_eq!(channel["email"]["security"], json!("starttls"));
    assert_eq!(channel["email"]["username"], json!("keel"));
    assert_eq!(channel["email"]["hasPassword"], json!(true));
    assert!(channel["email"].get("password").is_none(), "{channel}");
    assert_eq!(channel["createdBy"], json!("admin"));
    assert!(
        serde_json::to_string(channel)
            .expect("json")
            .contains("hunter2")
            .not()
    );

    harness
        .registry()
        .services()
        .get::<keel_system::audit::api::Flush>()
        .expect("the audit module provides Flush")
        .flush()
        .await
        .expect("the worker answers the flush");
    let audited = harness
        .database()
        .rows(&format!(
            "SELECT params_redacted::text FROM audit_call WHERE function = '{CHANNEL_SET}' AND status = 'committed' ORDER BY at"
        ))
        .await;
    assert_eq!(audited.len(), 1, "{audited:?}");
    let params: Value = serde_json::from_str(&audited[0]).expect("json");
    assert_eq!(params["email"]["password"], json!("***"));
    assert_eq!(params["email"]["username"], json!("keel"));
    assert!(!audited[0].contains("hunter2"), "{}", audited[0]);

    let edited = harness
        .call(
            Who::Admin,
            CHANNEL_SET,
            json!({ "id": id, "kind": "email", "name": "ops mail 2", "enabled": false, "email": email(2526, "tls", None) }),
        )
        .await;
    assert_eq!(data(&edited)["name"], json!("ops mail 2"));
    assert_eq!(data(&edited)["enabled"], json!(false));
    assert_eq!(data(&edited)["email"]["port"], json!(2526));
    assert_eq!(data(&edited)["email"]["hasPassword"], json!(true));
    let kept = harness
        .database()
        .rows(&format!(
            "SELECT config_json->>'password' FROM notify_channel WHERE id = '{id}'"
        ))
        .await;
    assert_eq!(kept, stored);
    let listed = harness.call(Who::Admin, CHANNEL_LIST, json!({})).await;
    assert_eq!(data(&listed)["total"], json!(1));
    assert_eq!(data(&listed)["data"][0]["name"], json!("ops mail 2"));
    let reseller = harness.call(Who::Reseller, CHANNEL_LIST, json!({})).await;
    refused(&reseller, 403, "keel.error.level");
    let deleted = harness
        .call(Who::Admin, CHANNEL_DELETE, json!({ "id": id }))
        .await;
    assert_eq!(data(&deleted)["deleted"], json!(true));
    let gone = harness
        .call(Who::Admin, CHANNEL_DELETE, json!({ "id": id }))
        .await;
    refused(&gone, 404, "keel.error.not_found");
    harness.finish().await;
}

trait Not {
    fn not(self) -> bool;
}

impl Not for bool {
    fn not(self) -> bool {
        !self
    }
}

#[tokio::test]
async fn a_template_renders_with_single_brace_placeholders_and_a_double_brace_fails_the_freeze() {
    let templates = keel_system::notify::shipped_templates();
    let ids: Vec<&str> = templates.iter().map(MailTemplate::id).collect();
    assert_eq!(
        ids,
        [KEY_PROBLEM_RAISED, "notify.problem.cleared", KEY_TEST]
    );
    for template in &templates {
        for lang in ["en", "ru"] {
            let text = template.text(lang).expect("both languages ship");
            assert!(!text.subject.contains("{{"), "{}", text.subject);
            assert!(text.subject.contains("{params."), "{}", text.subject);
        }
    }
    let error = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&DOUBLED)
        .try_boot()
        .await
        .expect_err("a template with double braces fails the start");
    let text = error.to_string();
    assert!(text.contains("module notify: start failed"), "{text}");
    assert!(text.contains(ERROR_TEMPLATE_PLACEHOLDER), "{text}");
    assert!(text.contains("doubled.hello"), "{text}");
    assert!(text.contains("{{"), "{text}");
    let error = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&COLLIDING)
        .try_boot()
        .await
        .expect_err("a colliding template id fails the start");
    let text = error.to_string();
    assert!(text.contains(ERROR_TEMPLATE_TWICE), "{text}");
    assert!(text.contains(KEY_TEST), "{text}");
    let harness = boot().await;
    let port = closed_port().await;
    let id = set_channel(&harness, "ops", true, email(port, "none", None)).await;
    let asleep = set_channel(&harness, "asleep", false, email(port, "none", None)).await;
    let (message, delivery) = test_channel(&harness, id).await;
    let row = wait_attempt(&harness, delivery, 1).await;
    assert_eq!(row["messageKey"], json!(KEY_TEST));
    assert_eq!(row["channel"], json!("ops"));
    let stored = harness
        .database()
        .rows(&format!(
            "SELECT key || ' ' || source || ' ' || (params_json->>'channel') FROM notify_message WHERE id = '{message}'"
        ))
        .await;
    assert_eq!(stored, [format!("{KEY_TEST} test ops")]);
    let cursor = harness
        .database()
        .rows(&format!(
            "SELECT seq::text FROM notify_cursor WHERE consumer = '{CURSOR}'"
        ))
        .await;
    let created = harness
        .database()
        .rows(&format!(
            "SELECT seq::text FROM keel_outbox WHERE type = 'notify.message.created' AND data_json->>'id' = '{message}'"
        ))
        .await;
    assert_eq!(
        cursor, created,
        "the test delivery is created past the outbox"
    );
    assert_eq!(
        deliveries(&harness).await,
        [format!("ops {KEY_TEST} pending 1")]
    );
    let (_, on_asleep) = test_channel(&harness, asleep).await;
    let row = wait_attempt(&harness, on_asleep, 1).await;
    assert_eq!(row["channel"], json!("asleep"));
    assert_eq!(deliveries(&harness).await.len(), 2);
    harness.finish().await;
}

#[tokio::test]
async fn a_message_becomes_a_delivery_per_channel_within_a_second_and_the_cursor_survives_a_restart()
 {
    let harness = boot().await;
    let port = closed_port().await;
    set_channel(&harness, "first", true, email(port, "none", None)).await;
    set_channel(&harness, "second", true, email(port, "none", None)).await;
    set_channel(&harness, "asleep", false, email(port, "none", None)).await;
    assert!(deliveries(&harness).await.is_empty());
    Problems::raise(
        caller(&harness).as_ref(),
        Raise::new("probe.disk", "/", Severity::Warning).source("probe.disk"),
    )
    .await
    .expect("the problem is raised");
    let elapsed = wait_until("a delivery per enabled channel", || async {
        deliveries(&harness).await.len() >= 2
    })
    .await;
    assert!(
        elapsed <= PROMPT,
        "the deliveries took {elapsed:?}; the consumer wakes on the commit"
    );
    let rows = deliveries(&harness).await;
    assert_eq!(rows.len(), 2);
    assert!(
        rows[0].starts_with(&format!("first {KEY_PROBLEM_RAISED} ")),
        "{rows:?}"
    );
    assert!(
        rows[1].starts_with(&format!("second {KEY_PROBLEM_RAISED} ")),
        "{rows:?}"
    );
    let newest = harness
        .database()
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    wait_until("the cursor reaches the newest event", || async {
        harness
            .database()
            .rows(&format!(
                "SELECT seq::text FROM notify_cursor WHERE consumer = '{CURSOR}'"
            ))
            .await
            == newest
    })
    .await;
    let messages_before = harness
        .database()
        .rows("SELECT count(*)::text FROM notify_message")
        .await;

    let database = harness.stop().await;
    let again = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .module(&keel_system::audit::MODULE)
        .clock(at(START))
        .database(database)
        .boot()
        .await;
    tokio::time::sleep(PROMPT).await;
    assert_eq!(
        deliveries(&again).await.len(),
        2,
        "a restart creates no duplicate delivery"
    );
    assert_eq!(
        again
            .database()
            .rows("SELECT count(*)::text FROM notify_message")
            .await,
        messages_before
    );
    Problems::raise(
        caller(&again).as_ref(),
        Raise::new("probe.disk", "/var", Severity::Warning).source("probe.disk"),
    )
    .await
    .expect("the problem is raised");
    wait_until("the deliveries of the second problem", || async {
        deliveries(&again).await.len() >= 4
    })
    .await;
    let cursor = again
        .database()
        .rows(&format!(
            "SELECT seq::text FROM notify_cursor WHERE consumer = '{CURSOR}'"
        ))
        .await;
    let newest = again
        .database()
        .rows("SELECT max(seq)::text FROM keel_outbox")
        .await;
    assert_eq!(cursor, newest);
    let listed = again
        .call(Who::Admin, DELIVERY_LIST, json!({ "search": "first" }))
        .await;
    assert_eq!(data(&listed)["total"], json!(2));
    let by_state = again
        .call(
            Who::Admin,
            DELIVERY_LIST,
            json!({ "filters": [{ "key": "state", "op": "eq", "value": STATE_SENT }] }),
        )
        .await;
    assert_eq!(data(&by_state)["total"], json!(0));
    again.finish().await;
}

#[tokio::test]
async fn a_refused_smtp_connection_follows_the_ladder_and_the_fifth_failure_raises_notify_delivery_failed()
 {
    let harness = boot().await;
    let port = closed_port().await;
    let channel = set_channel(&harness, "refusing", true, email(port, "none", None)).await;
    let (_, delivery) = test_channel(&harness, channel).await;
    let mut now = at(START);
    for attempt in 1..MAX_ATTEMPTS {
        let row = wait_attempt(&harness, delivery, i64::from(attempt)).await;
        assert_eq!(row["attempt"], json!(attempt), "{row}");
        assert_eq!(row["state"], json!(STATE_PENDING), "{row}");
        let error = row["error"].as_str().expect("an error text");
        assert!(
            error.contains("notify.error.email_send"),
            "attempt {attempt}: {error}"
        );
        assert!(
            !error.to_lowercase().contains("provider"),
            "attempt {attempt}: {error}"
        );
        let wait = LADDER[usize::try_from(attempt - 1).expect("an index")];
        let expected = now + keel_sdk::chrono::Duration::from_std(wait).expect("a delta");
        assert_eq!(row["nextAt"], json!(expected), "attempt {attempt}");
        wait_until("the channel records the error", || async {
            data(&channel_row(&harness, channel).await)["lastError"] == json!(error)
        })
        .await;
        assert_eq!(
            data(&channel_row(&harness, channel).await)["lastSentAt"],
            Value::Null
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            delivery_row(&harness, delivery).await["attempt"],
            json!(attempt),
            "no attempt before the clock reaches nextAt"
        );
        now = expected;
        harness.clock().set(now);
    }
    let row = wait_attempt(&harness, delivery, i64::from(MAX_ATTEMPTS)).await;
    assert_eq!(row["state"], json!(STATE_FAILED));
    assert_eq!(row["attempt"], json!(MAX_ATTEMPTS));
    wait_until("the dead letter is raised", || async {
        !harness
            .database()
            .rows(&format!(
                "SELECT id FROM notify_problem WHERE key = '{DELIVERY_FAILED}' AND cleared_at IS NULL"
            ))
            .await
            .is_empty()
    })
    .await;
    let problems = harness
        .database()
        .rows(&format!(
            "SELECT key || ' ' || subject || ' ' || severity || ' ' || source || ' ' || (params_json->>'channel') FROM notify_problem WHERE key = '{DELIVERY_FAILED}'"
        ))
        .await;
    assert_eq!(
        problems,
        [format!(
            "{DELIVERY_FAILED} {channel} warning notify.delivery refusing"
        )]
    );
    let admin_messages = harness
        .database()
        .rows(&format!(
            "SELECT key || ' ' || source FROM notify_message WHERE principal_id = '{}' ORDER BY created_at, id",
            harness.principal(Who::Admin).id
        ))
        .await;
    assert_eq!(
        admin_messages,
        [
            format!("{KEY_TEST} test"),
            format!("{KEY_PROBLEM_RAISED} problem")
        ]
    );
    tokio::time::sleep(PROMPT).await;
    let failed = harness
        .call(
            Who::Admin,
            DELIVERY_LIST,
            json!({ "filters": [{ "key": "state", "op": "eq", "value": STATE_FAILED }] }),
        )
        .await;
    assert_eq!(data(&failed)["total"], json!(1));
    let pending = harness
        .call(
            Who::Admin,
            DELIVERY_LIST,
            json!({ "filters": [{ "key": "state", "op": "eq", "value": STATE_PENDING }] }),
        )
        .await;
    assert_eq!(
        data(&pending)["total"],
        json!(1),
        "the dead-letter notification queues one delivery on the failing channel"
    );
    assert_eq!(
        data(&pending)["data"][0]["messageKey"],
        json!(KEY_PROBLEM_RAISED)
    );
    harness.finish().await;
}

#[tokio::test]
async fn a_tls_channel_to_a_closed_port_fails_on_the_connection_and_not_for_want_of_a_provider() {
    let harness = boot().await;
    let port = closed_port().await;
    for (name, security) in [("implicit", "tls"), ("upgraded", "starttls")] {
        let channel = set_channel(&harness, name, true, email(port, security, Some("pw"))).await;
        let (_, delivery) = test_channel(&harness, channel).await;
        let row = wait_attempt(&harness, delivery, 1).await;
        let error = row["error"].as_str().expect("an error text");
        let lower = error.to_lowercase();
        assert!(
            lower.contains("connection refused") || lower.contains("connect"),
            "{name}: {error}"
        );
        assert!(!lower.contains("provider"), "{name}: {error}");
        assert!(!lower.contains("crypto"), "{name}: {error}");
        assert_eq!(row["state"], json!(STATE_PENDING));
    }
    harness.finish().await;
}

#[tokio::test]
async fn the_channel_and_delivery_descriptors_match_their_snapshots() {
    let harness = boot().await;
    for level in Level::ALL {
        let names = harness.snapshot_descriptors_of(level, snapshots_dir(), MODULE.id);
        let channels: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|name| {
                name.starts_with("notify.channel.") || name.starts_with("notify.delivery.")
            })
            .collect();
        if level < Level::Admin {
            assert!(channels.is_empty(), "{level}: {channels:?}");
        } else {
            assert_eq!(
                channels,
                [
                    CHANNEL_DELETE,
                    CHANNEL_GET,
                    CHANNEL_LIST,
                    CHANNEL_SET,
                    CHANNEL_TEST,
                    DELIVERY_LIST,
                ],
                "{level}"
            );
        }
    }
    harness.finish().await;
}
