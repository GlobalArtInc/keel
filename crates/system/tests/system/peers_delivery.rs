use std::future::Future;
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::serde_json;
use keel_sdk::system::{EventRelay, SecretCipher};
use keel_sdk::types::{Event, EventSubject};
use keel_system::peers::{
    CREATE, DELETE, ERROR_EVENT, ERROR_FEED, ERROR_MASK, ERROR_QUERY, ERROR_SIGNATURE, FEED_PATH,
    HOOK_PATH, INBOX_MAX_ATTEMPTS, INBOX_RETENTION_DAYS,
};
use keel_system::webhooks::api::{EVENT_ID_HEADER, PEER_HEADER, SIGNATURE_HEADER, sign};
use keel_testkit::{
    Answer, EVENTS_PATH, EventStream, Harness, PRINCIPAL_HEADER, TestHttp, Who, tokio,
};

use crate::peers::{Linked, Two, composed, mint, outbound, refused, run_check};

const MASK: &str = "probe.note.*";
const SAVED: &str = "probe.note.saved";
const TAGGED: &str = "probe.tag.added";
const SAVE: &str = "probe.note.save";
const TAG: &str = "probe.tag.add";
const INVALIDATE: &str = "data.invalidate";
const PATIENCE: Duration = Duration::from_secs(40);
const QUIET: Duration = Duration::from_millis(700);
const SECRET: &str = "whs_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd";

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
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn emitting(name: &'static str, kind: &'static str) -> FunctionDef {
    function::<Save, Nothing>(name)
        .kind(Kind::Action)
        .level(Level::User)
        .unowned()
        .key("id")
        .handler(move |call: Call<Save>| async move {
            call.resolve_account(call.params().account).await?;
            call.emit(kind, &json!({ "id": call.params().id }))?;
            Ok(Nothing {})
        })
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(emitting(SAVE, SAVED))
        .function(emitting(TAG, TAGGED));
    Ok(())
}

async fn neighbour() -> Harness {
    composed(Harness::new().module(&PROBE)).boot().await
}

async fn behind_nat() -> Harness {
    composed(Harness::new()).boot().await
}

async fn until<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness
        .db()
        .query_all(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .iter()
        .map(|row| {
            row.try_get_by_index::<Option<String>>(0)
                .unwrap_or_else(|error| panic!("{sql}: {error}"))
                .unwrap_or_default()
        })
        .collect()
}

async fn until_rows(harness: &Harness, sql: &str, expected: &[String]) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let found = rows(harness, sql).await;
        if found == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{sql}: {found:?} instead of {expected:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn execute(harness: &Harness, sql: &str) {
    harness
        .db()
        .execute_unprepared(sql)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn emit(harness: &Harness, who: Who, func: &str) -> String {
    let note = Id::generate();
    let reply = harness.call(who, func, json!({ "id": note })).await;
    assert_eq!(reply.status(), 200, "{reply:?}");
    let ids = rows(
        harness,
        &format!("SELECT id FROM keel_outbox WHERE data_json->>'id' = '{note}'"),
    )
    .await;
    assert_eq!(ids.len(), 1, "{ids:?}");
    ids[0].clone()
}

async fn seq_of(harness: &Harness, event: &str) -> u64 {
    rows(
        harness,
        &format!("SELECT seq::text FROM keel_outbox WHERE id = '{event}'"),
    )
    .await[0]
        .parse()
        .unwrap()
}

async fn newest(harness: &Harness) -> u64 {
    rows(
        harness,
        "SELECT COALESCE(max(seq), 0)::text FROM keel_outbox",
    )
    .await[0]
        .parse()
        .unwrap()
}

async fn until_cursor(a: &Harness, b: &Harness) {
    until(
        "the cursor reaches the newest event of the neighbour",
        || async {
            rows(a, "SELECT pull_seq::text FROM peer_product").await
                == [newest(b).await.to_string()]
        },
    )
    .await;
}

async fn processed(harness: &Harness) -> Vec<String> {
    rows(
        harness,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL ORDER BY event_id",
    )
    .await
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn cipher(harness: &Harness) -> Arc<dyn SecretCipher> {
    harness
        .registry()
        .services()
        .get::<dyn SecretCipher>()
        .expect("the kernel seals with the data key")
}

fn relay(harness: &Harness) -> Arc<dyn EventRelay> {
    harness
        .registry()
        .services()
        .get::<dyn EventRelay>()
        .expect("the kernel relays events of peers")
}

fn foreign(instance: Id, kind: &str, account: Id) -> Event {
    Event {
        id: Id::generate(),
        seq: 1,
        kind: kind.to_owned(),
        at: keel_sdk::chrono::Utc::now(),
        instance_id: instance,
        module: "probe".into(),
        actor: None,
        subject: Some(EventSubject {
            kind: "probe.note".into(),
            id: Id::generate(),
            account_id: Some(account),
        }),
        data: json!({ "id": Id::generate() }),
    }
}

async fn pair_behind_nat(level: &str, scopes: Value) -> (Two, Linked) {
    let two = Two::of(behind_nat().await, neighbour().await).await;
    let linked = two
        .pair(
            json!({ "scopes": scopes, "level": level, "onBehalf": true }),
            json!({ "subscribe": [MASK] }),
        )
        .await;
    (two, linked)
}

async fn hook(
    http: &TestHttp,
    peer: &str,
    event_id: &str,
    signature: Option<&str>,
    body: &[u8],
) -> Answer {
    let mut headers = vec![
        ("content-type", "application/json"),
        (PEER_HEADER, peer),
        (EVENT_ID_HEADER, event_id),
    ];
    if let Some(signature) = signature {
        headers.push((SIGNATURE_HEADER, signature));
    }
    match http
        .try_request_bytes("POST", HOOK_PATH, &headers, body)
        .await
    {
        Ok(answer) => answer,
        Err(error) => panic!("{error}"),
    }
}

async fn feed(http: &TestHttp, token: &str, query: &str) -> Answer {
    let bearer = format!("Bearer {token}");
    http.request(
        "GET",
        &format!("{FEED_PATH}{query}"),
        &[("authorization", bearer.as_str())],
        None,
    )
    .await
}

fn served(answer: &Answer) -> Vec<String> {
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["id"].as_str().unwrap().to_owned())
        .collect()
}

fn next(answer: &Answer) -> u64 {
    answer.json()["next"].as_u64().unwrap()
}

async fn sealed_secret(harness: &Harness, link: &str, secret: &str) {
    let sealed = cipher(harness).seal(secret.as_bytes()).unwrap();
    execute(
        harness,
        &format!("UPDATE peer_product SET webhook_secret_enc = '{sealed}' WHERE id = '{link}'"),
    )
    .await;
}

fn event_of(instance: Id) -> (String, Vec<u8>) {
    let event = foreign(instance, SAVED, Id::ROOT_ACCOUNT);
    (
        event.id.to_string(),
        serde_json::to_vec(&event).expect("an event serialises"),
    )
}

#[cfg(feature = "settings")]
async fn with_an_address() -> Harness {
    composed(Harness::new().module(&keel_system::settings::MODULE))
        .boot()
        .await
}

#[cfg(feature = "settings")]
async fn give_the_address(two: &Two) {
    let set = two
        .a
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::auth::SETTING_SHELL_URL, "value": two.a_http.url("") }),
        )
        .await;
    assert_eq!(set.status(), 200, "{set:?}");
}

#[cfg(feature = "settings")]
async fn pair_with_an_address(level: &str, a_grants: Value) -> (Two, Linked) {
    let two = Two::of(with_an_address().await, neighbour().await).await;
    give_the_address(&two).await;
    let linked = two
        .pair(json!({ "scopes": [], "level": level }), a_grants)
        .await;
    (two, linked)
}

#[cfg(feature = "settings")]
async fn pair_with_a_hook(level: &str) -> (Two, Linked) {
    pair_with_an_address(level, json!({ "subscribe": [MASK] })).await
}

async fn second_link(harness: &Harness, instance: Id) -> String {
    let id = Id::generate().to_string();
    let sealed = cipher(harness).seal(SECRET.as_bytes()).unwrap();
    execute(
        harness,
        &format!(
            "INSERT INTO peer_product (id, instance_id, products_json, keel_version, protocol, url, \
             public_key, inbound_token_hash, inbound_scopes_json, outbound_token_enc, \
             webhook_secret_enc, status, created_at) VALUES ('{id}', '{instance}', '[]', '0.3.0', 1, \
             NULL, 'ed25519:{id}', 'hash-{id}', '{{\"functions\": [], \"level\": \"service\", \"onBehalf\": false}}', \
             'sealed-{id}', '{sealed}', 'active', now())"
        ),
    )
    .await;
    id
}

async fn deliver(harness: &Harness, http: &TestHttp, event: &Value) -> Answer {
    let body = serde_json::to_vec(event).expect("an event serialises");
    let signature = sign(SECRET.as_bytes(), harness.clock().now().timestamp(), &body).unwrap();
    hook(
        http,
        event["instanceId"].as_str().unwrap(),
        event["id"].as_str().unwrap(),
        Some(&signature),
        &body,
    )
    .await
}

fn as_value(event: &Event) -> Value {
    serde_json::to_value(event).expect("an event serialises")
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn an_event_arrives_signed_and_a_repeat_creates_no_duplicate() {
    let (two, linked) = pair_with_a_hook("admin").await;
    let subscriptions = rows(
        &two.b,
        "SELECT peer_id || ' ' || principal_id || ' ' || level || ' ' || url || ' ' || array_to_string(events, ',') FROM webhook_subscription",
    )
    .await;
    let principal = rows(
        &two.b,
        &format!(
            "SELECT principal_id FROM peer_product WHERE id = '{}'",
            linked.b_link
        ),
    )
    .await;
    assert_eq!(
        subscriptions,
        [format!(
            "{} {} admin {}{HOOK_PATH} {MASK}",
            linked.b_link,
            principal[0],
            two.a_http.url("")
        )]
    );
    assert_eq!(
        rows(
            &two.a,
            "SELECT (pull_seq IS NULL AND webhook_secret_enc IS NOT NULL)::text FROM peer_product"
        )
        .await,
        ["true"]
    );

    let event = emit(&two.b, Who::Admin, SAVE).await;
    until_rows(
        &two.a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL",
        std::slice::from_ref(&event),
    )
    .await;
    assert_eq!(
        rows(&two.a, "SELECT peer_id FROM peer_inbox").await,
        std::slice::from_ref(&linked.a_link)
    );
    until_rows(
        &two.b,
        "SELECT state || ' ' || attempt || ' ' || last_status FROM webhook_delivery",
        &["delivered 1 200".to_owned()],
    )
    .await;
    let subscriber = two
        .a
        .registry()
        .services()
        .get::<dyn keel_sdk::system::OutboxSubscriber>()
        .unwrap();
    let on_a: Vec<Event> = subscriber
        .after(0, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|seen| seen.id.to_string() == event)
        .collect();
    assert_eq!(on_a.len(), 1);
    assert_eq!(on_a[0].instance_id, two.b.instance());
    assert_eq!(on_a[0].kind, SAVED);
    assert_eq!(on_a[0].module, "probe");
    let on_b: Vec<Event> = two
        .b
        .registry()
        .services()
        .get::<dyn keel_sdk::system::OutboxSubscriber>()
        .unwrap()
        .after(0, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|seen| seen.id.to_string() == event)
        .collect();
    assert_eq!(on_a[0].data, on_b[0].data);
    assert_eq!(on_a[0].subject, on_b[0].subject);
    assert_eq!(on_a[0].at, on_b[0].at);

    execute(
        &two.b,
        "UPDATE webhook_delivery SET state = 'pending', next_at = '2000-01-01T00:00:00Z'",
    )
    .await;
    until_rows(
        &two.b,
        "SELECT state || ' ' || attempt || ' ' || last_status FROM webhook_delivery",
        &["delivered 2 200".to_owned()],
    )
    .await;
    let secret = rows(&two.a, "SELECT webhook_secret_enc FROM peer_product").await;
    let secret = cipher(&two.a).open(&secret[0]).unwrap();
    let body = serde_json::to_vec(&on_b[0]).unwrap();
    let signature = sign(&secret, two.a.clock().now().timestamp(), &body).unwrap();
    let repeated = hook(
        &two.a_http,
        &two.b.instance().to_string(),
        &event,
        Some(&signature),
        &body,
    )
    .await;
    assert_eq!(repeated.status, 200, "{}", repeated.body);
    assert_eq!(
        repeated.json(),
        json!({ "data": { "eventId": event, "duplicate": true } })
    );
    assert_eq!(
        rows(&two.a, "SELECT count(*)::text FROM peer_inbox").await,
        ["1"]
    );
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT count(*)::text || ' ' || min(instance_id) FROM keel_outbox WHERE id = '{event}'")
        )
        .await,
        [format!("1 {}", two.b.instance())]
    );
    assert_eq!(processed(&two.a).await, [event]);
    two.finish().await;
}

#[tokio::test]
async fn a_bad_or_stale_signature_is_refused() {
    let (two, linked) = pair_behind_nat("admin", json!([])).await;
    sealed_secret(&two.a, &linked.a_link, SECRET).await;
    let peer = two.b.instance().to_string();
    let now = two.a.clock().now().timestamp();
    let (id, body) = event_of(two.b.instance());
    let good = sign(SECRET.as_bytes(), now, &body).unwrap();
    let mut tampered = body.clone();
    tampered.extend_from_slice(b" ");
    let other = sign(b"whs_another_secret_of_another_link", now, &body).unwrap();
    let stale = sign(SECRET.as_bytes(), now - 301, &body).unwrap();
    let ahead = sign(SECRET.as_bytes(), now + 301, &body).unwrap();
    let unknown = Id::generate().to_string();
    let earliest = format!("t={},v1=00", i64::MIN);
    let latest = format!("t={},v1=00", i64::MAX);
    let cases: [(&str, &str, Option<&str>, &[u8]); 9] = [
        ("unsigned", &peer, None, &body),
        ("another secret", &peer, Some(&other), &body),
        ("another body", &peer, Some(&good), &tampered),
        ("stale", &peer, Some(&stale), &body),
        ("ahead", &peer, Some(&ahead), &body),
        ("malformed", &peer, Some("t=abc,v1=00"), &body),
        ("the earliest time", &peer, Some(&earliest), &body),
        ("the latest time", &peer, Some(&latest), &body),
        ("unknown peer", &unknown, Some(&good), &body),
    ];
    for (case, peer, signature, body) in cases {
        let answer = hook(&two.a_http, peer, &id, signature, body).await;
        assert_eq!(answer.status, 401, "{case}: {}", answer.body);
        assert_eq!(
            answer.json()["error"]["key"],
            json!(ERROR_SIGNATURE),
            "{case}"
        );
    }
    assert_eq!(
        rows(&two.a, "SELECT count(*)::text FROM peer_inbox").await,
        ["0"]
    );
    let edge = sign(SECRET.as_bytes(), now - 300, &body).unwrap();
    let accepted = hook(&two.a_http, &peer, &id, Some(&edge), &body).await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    assert_eq!(
        accepted.json(),
        json!({ "data": { "eventId": id, "duplicate": false } })
    );
    until(&format!("{id} is processed"), || async {
        processed(&two.a).await == [id.clone()]
    })
    .await;
    two.finish().await;
}

#[tokio::test]
async fn a_signed_delivery_must_be_an_event_of_the_installation_that_signed_it() {
    let (two, linked) = pair_behind_nat("admin", json!([])).await;
    let peer = two.b.instance().to_string();
    let now = two.a.clock().now().timestamp();
    let (id, body) = event_of(two.b.instance());
    let good = sign(SECRET.as_bytes(), now, &body).unwrap();
    let pulled = hook(&two.a_http, &peer, &id, Some(&good), &body).await;
    refused(&pulled, 401, ERROR_SIGNATURE);
    sealed_secret(&two.a, &linked.a_link, SECRET).await;
    let other_id = Id::generate().to_string();
    let renamed = hook(&two.a_http, &peer, &other_id, Some(&good), &body).await;
    refused(&renamed, 422, ERROR_EVENT);
    assert_eq!(renamed.json()["error"]["params"]["reason"], json!("id"));
    let (stranger, strange) = event_of(Id::generate());
    let signed = sign(SECRET.as_bytes(), now, &strange).unwrap();
    let relayed = hook(&two.a_http, &peer, &stranger, Some(&signed), &strange).await;
    refused(&relayed, 422, ERROR_EVENT);
    assert_eq!(
        relayed.json()["error"]["params"]["reason"],
        json!("instance")
    );
    let garbage = b"{\"not\": \"an event\"}";
    let signed = sign(SECRET.as_bytes(), now, garbage).unwrap();
    let unreadable = hook(&two.a_http, &peer, &id, Some(&signed), garbage).await;
    refused(&unreadable, 422, ERROR_EVENT);
    assert_eq!(
        unreadable.json()["error"]["params"]["reason"],
        json!("body")
    );
    assert_eq!(
        rows(&two.a, "SELECT count(*)::text FROM peer_inbox").await,
        ["0"]
    );
    two.finish().await;
}

#[tokio::test]
async fn the_pull_feed_resumes_after_its_cursor_behind_nat() {
    let a = behind_nat().await;
    let b = neighbour().await;
    let before = emit(&b, Who::Admin, SAVE).await;
    let two = Two::of(a, b).await;
    let linked = two
        .pair(
            json!({ "scopes": [] , "level": "admin" }),
            json!({ "subscribe": [MASK] }),
        )
        .await;
    assert_eq!(
        rows(
            &two.a,
            "SELECT (pull_seq = 0 AND url IS NOT NULL AND webhook_secret_enc IS NULL)::text FROM peer_product"
        )
        .await,
        ["true"]
    );
    assert_eq!(
        rows(
            &two.b,
            &format!(
                "SELECT array_to_string(events, ',') || ' ' || (feed_seq >= {})::text || ' ' || (url IS NULL)::text FROM peer_product",
                seq_of(&two.b, &before).await
            )
        )
        .await,
        [format!("{MASK} true true")]
    );
    assert_eq!(
        rows(&two.b, "SELECT count(*)::text FROM webhook_subscription").await,
        ["0"]
    );
    let first = emit(&two.b, Who::Admin, SAVE).await;
    let second = emit(&two.b, Who::Admin, SAVE).await;
    let pulled = sorted(vec![first.clone(), second.clone()]);
    until_rows(
        &two.a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL ORDER BY event_id",
        &pulled,
    )
    .await;
    until_cursor(&two.a, &two.b).await;
    assert_eq!(
        rows(
            &two.a,
            &format!(
                "SELECT count(*)::text FROM keel_outbox WHERE instance_id = '{}'",
                two.b.instance()
            )
        )
        .await,
        ["2"]
    );

    let Two {
        a,
        b,
        a_http,
        b_http,
    } = two;
    a_http.stop().await.unwrap();
    execute(&a, "DELETE FROM peer_inbox").await;
    let key = a.key().clone();
    let instance = a.instance();
    let database = a.stop().await;
    let third = emit(&b, Who::Admin, SAVE).await;
    let a = composed(
        Harness::new()
            .key(key)
            .instance(instance)
            .database(database),
    )
    .boot()
    .await;
    until_rows(
        &a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL ORDER BY event_id",
        std::slice::from_ref(&third),
    )
    .await;
    until_cursor(&a, &b).await;
    assert_eq!(
        rows(&a, "SELECT event_id FROM peer_inbox").await,
        [third],
        "the feed resumed after the stored cursor instead of reading {first} and {second} again"
    );
    assert_eq!(
        rows(
            &a,
            &format!("SELECT count(*)::text FROM peer_inbox WHERE event_id = '{before}'")
        )
        .await,
        ["0"]
    );
    assert_eq!(
        rows(&a, "SELECT id FROM peer_product").await,
        [linked.a_link]
    );
    b_http.stop().await.unwrap();
    a.finish().await;
    b.finish().await;
}

#[tokio::test]
async fn the_feed_serves_the_subscribed_events_of_this_installation_since_pairing_within_the_link_level()
 {
    let a = behind_nat().await;
    let b = neighbour().await;
    let before = emit(&b, Who::Admin, SAVE).await;
    let two = Two::of(a, b).await;
    let linked = two
        .pair(
            json!({ "scopes": [], "level": "service" }),
            json!({ "subscribe": [MASK] }),
        )
        .await;
    let token = outbound(&two.a, &linked.a_link).await;
    let root = emit(&two.b, Who::Admin, SAVE).await;
    emit(&two.b, Who::Admin, TAG).await;
    let customer = emit(&two.b, Who::A1, SAVE).await;
    let third = Id::generate();
    relay(&two.b)
        .relay(&foreign(third, SAVED, Id::ROOT_ACCOUNT))
        .await
        .unwrap()
        .expect("a foreign event enters the outbox of b");
    let top = newest(&two.b).await;

    let page = feed(&two.b_http, &token, "").await;
    assert_eq!(served(&page), std::slice::from_ref(&root));
    assert_eq!(next(&page), top);
    assert!(!served(&page).contains(&before));

    execute(
        &two.b,
        &format!(
            "UPDATE peer_product SET inbound_scopes_json = '{{\"functions\": [], \"level\": \"admin\", \"onBehalf\": false}}' WHERE id = '{}'",
            linked.b_link
        ),
    )
    .await;
    let page = feed(&two.b_http, &token, "").await;
    assert_eq!(served(&page), [root.clone(), customer.clone()]);
    let first = feed(&two.b_http, &token, &format!("?types={SAVED}&limit=1")).await;
    assert_eq!(served(&first), std::slice::from_ref(&root));
    assert_eq!(next(&first), seq_of(&two.b, &root).await);
    let rest = feed(
        &two.b_http,
        &token,
        &format!("?after={}&limit=1", next(&first)),
    )
    .await;
    assert_eq!(served(&rest), std::slice::from_ref(&customer));
    let tags = feed(&two.b_http, &token, "?types=probe.tag.*").await;
    assert!(served(&tags).is_empty());
    assert_eq!(next(&tags), top);
    let caught_up = feed(&two.b_http, &token, &format!("?after={top}")).await;
    assert!(served(&caught_up).is_empty());
    assert_eq!(next(&caught_up), top);
    let early = feed(&two.b_http, &token, "?after=0&limit=500").await;
    assert_eq!(served(&early), [root, customer]);

    let reverse = outbound(&two.b, &linked.b_link).await;
    emit(&two.b, Who::Admin, SAVE).await;
    let initiator = feed(&two.a_http, &reverse, "").await;
    assert!(served(&initiator).is_empty(), "{}", initiator.body);
    two.finish().await;
}

#[tokio::test]
async fn the_feed_answers_the_peer_token_of_a_link_alone() {
    let (two, linked) = pair_behind_nat("admin", json!(["auth.token.create"])).await;
    let token = outbound(&two.a, &linked.a_link).await;
    let guest = two.b_http.get(Who::Guest, FEED_PATH).await;
    refused(&guest, 401, "keel.error.unauthenticated");
    let admin = two.b_http.get(Who::Admin, FEED_PATH).await;
    refused(&admin, 403, ERROR_FEED);
    let minted = crate::peers::mint(&two.b_http, &token).await;
    refused(&feed(&two.b_http, &minted, "").await, 403, ERROR_FEED);
    let origin = format!("bill:{}", two.a.instance());
    let customer = two.b.principal(Who::A1).clone();
    execute(
        &two.b,
        &format!(
            "UPDATE keel_principal SET origin = '{origin}', external_id = 'client-1' WHERE id = '{}'",
            customer.id
        ),
    )
    .await;
    execute(
        &two.b,
        &format!(
            "UPDATE keel_account SET origin = '{origin}', external_id = 'acc-1' WHERE id = '{}'",
            customer.account_id
        ),
    )
    .await;
    let bearer = format!("Bearer {token}");
    let header = format!("origin={origin}; external_id=client-1");
    let on_behalf = two
        .b_http
        .request(
            "GET",
            FEED_PATH,
            &[
                ("authorization", bearer.as_str()),
                ("x-keel-on-behalf-of", header.as_str()),
            ],
            None,
        )
        .await;
    refused(&on_behalf, 403, ERROR_FEED);
    let malformed = feed(&two.b_http, &token, "?limit=0").await;
    assert_eq!(malformed.status, 422, "{}", malformed.body);
    assert_eq!(
        malformed.json()["error"]["fields"][0]["key"],
        json!(ERROR_QUERY)
    );
    assert_eq!(
        malformed.json()["error"]["fields"][0]["path"],
        json!("limit")
    );
    let mask = feed(&two.b_http, &token, "?types=probe").await;
    assert_eq!(mask.status, 422, "{}", mask.body);
    assert_eq!(mask.json()["error"]["fields"][0]["key"], json!(ERROR_MASK));
    assert_eq!(feed(&two.b_http, &token, "").await.status, 200);
    let deleted = two
        .b
        .call(Who::Admin, DELETE, json!({ "id": linked.b_link }))
        .await;
    assert_eq!(deleted.status(), 200, "{deleted:?}");
    refused(
        &feed(&two.b_http, &token, "").await,
        401,
        "keel.error.unauthenticated",
    );
    two.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn a_peer_subscription_changes_only_with_its_link_within_its_level_and_relays_nothing() {
    let (two, linked) = pair_with_a_hook("service").await;
    let subscription = rows(&two.b, "SELECT id FROM webhook_subscription").await;
    assert_eq!(subscription.len(), 1);
    let id = subscription[0].clone();
    let listed = two
        .b
        .call(
            Who::Admin,
            keel_system::webhooks::SUBSCRIPTION_LIST,
            json!({}),
        )
        .await;
    assert_eq!(
        crate::peers::data(&listed)["data"][0]["name"],
        json!(format!("peer-{}", linked.b_link.to_lowercase()))
    );
    for (func, params) in [
        (
            keel_system::webhooks::SUBSCRIPTION_SET,
            json!({ "id": id, "name": "mine", "url": "https://hooks.example/in", "events": ["probe.*"] }),
        ),
        (
            keel_system::webhooks::SUBSCRIPTION_TOGGLE,
            json!({ "id": id, "enabled": false }),
        ),
        (
            keel_system::webhooks::SUBSCRIPTION_DELETE,
            json!({ "id": id }),
        ),
    ] {
        let reply = two.b.call(Who::Admin, func, params).await;
        let error = reply.error().unwrap_or_else(|| panic!("{func}: {reply:?}"));
        assert_eq!(error.key, keel_system::webhooks::ERROR_PEER, "{func}");
        assert_eq!(reply.status(), 409, "{func}");
    }

    let stranger = foreign(Id::generate(), SAVED, Id::ROOT_ACCOUNT);
    let relayed = relay(&two.b)
        .relay(&stranger)
        .await
        .unwrap()
        .expect("a foreign event enters the outbox of b");
    let customer = emit(&two.b, Who::A1, SAVE).await;
    let root = emit(&two.b, Who::Admin, SAVE).await;
    let last = seq_of(&two.b, &root).await;
    assert!(relayed < last);
    until(&format!("the webhook consumer passes {last}"), || async {
        rows(&two.b, "SELECT seq::text FROM webhook_cursor")
            .await
            .first()
            .and_then(|seq| seq.parse::<u64>().ok())
            .is_some_and(|seq| seq >= last)
    })
    .await;
    assert_eq!(
        rows(&two.b, "SELECT event_id FROM webhook_delivery").await,
        std::slice::from_ref(&root)
    );
    until_rows(
        &two.a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL",
        &[root],
    )
    .await;
    assert!(!processed(&two.a).await.contains(&customer));
    assert!(!processed(&two.a).await.contains(&stranger.id.to_string()));

    let deleted = two
        .b
        .call(Who::Admin, DELETE, json!({ "id": linked.b_link }))
        .await;
    assert_eq!(deleted.status(), 200, "{deleted:?}");
    assert_eq!(
        rows(
            &two.b,
            "SELECT (SELECT count(*) FROM webhook_subscription) || ' ' || (SELECT count(*) FROM webhook_delivery)"
        )
        .await,
        ["0 0"]
    );
    two.finish().await;
}

#[tokio::test]
async fn an_event_mask_of_a_subscription_names_a_module_and_an_event() {
    let harness = behind_nat().await;
    let refused_mask = harness
        .call(
            Who::Admin,
            CREATE,
            json!({ "url": "http://127.0.0.1:9", "code": "PAIR-0000-0000", "subscribe": [MASK, "probe"] }),
        )
        .await;
    let error = refused_mask
        .error()
        .unwrap_or_else(|| panic!("{refused_mask:?}"));
    assert_eq!(error.fields[0].path, "subscribe[1]", "{refused_mask:?}");
    assert_eq!(error.fields[0].key.as_deref(), Some(ERROR_MASK));
    harness.finish().await;
}

#[tokio::test]
async fn the_daily_check_prunes_processed_inbox_rows_after_their_retention_and_those_of_links_gone()
{
    let harness = behind_nat().await;
    let link = second_link(&harness, Id::generate()).await;
    let now = harness.clock().now();
    let row = |id: &str, peer: &str, received: i64, processed: Option<i64>| {
        let received = (now - keel_sdk::chrono::Duration::days(received)).to_rfc3339();
        let processed = processed.map_or("NULL".to_owned(), |days| {
            format!(
                "'{}'",
                (now - keel_sdk::chrono::Duration::days(days)).to_rfc3339()
            )
        });
        format!("('{id}', '{peer}', '{{}}', '{received}', {processed})")
    };
    execute(
        &harness,
        &format!(
            "INSERT INTO peer_inbox (event_id, peer_id, event_json, received_at, processed_at) VALUES {}, {}, {}, {}",
            row("old", &link, INBOX_RETENTION_DAYS + 2, Some(INBOX_RETENTION_DAYS + 1)),
            row("recent", &link, INBOX_RETENTION_DAYS, Some(INBOX_RETENTION_DAYS - 1)),
            row("waiting", &link, INBOX_RETENTION_DAYS + 30, None),
            row("orphan", &Id::generate().to_string(), 1, None),
        ),
    )
    .await;
    run_check(&harness).await;
    assert_eq!(
        rows(
            &harness,
            "SELECT event_id FROM peer_inbox ORDER BY event_id"
        )
        .await,
        ["recent", "waiting"]
    );
    harness.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn an_initiator_with_an_address_may_choose_the_pull_feed_and_gets_no_subscription() {
    let (two, linked) =
        pair_with_an_address("admin", json!({ "subscribe": [MASK], "pull": true })).await;
    assert_eq!(
        rows(
            &two.a,
            "SELECT (pull_seq = 0 AND url IS NOT NULL AND webhook_secret_enc IS NULL)::text FROM peer_product"
        )
        .await,
        ["true"]
    );
    assert_eq!(
        rows(
            &two.b,
            "SELECT array_to_string(events, ',') || ' ' || (url IS NULL)::text || ' ' || (webhook_secret_enc IS NULL)::text FROM peer_product"
        )
        .await,
        [format!("{MASK} true true")]
    );
    assert_eq!(
        rows(&two.b, "SELECT count(*)::text FROM webhook_subscription").await,
        ["0"]
    );
    let event = emit(&two.b, Who::Admin, SAVE).await;
    until_rows(
        &two.a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL",
        std::slice::from_ref(&event),
    )
    .await;
    until_cursor(&two.a, &two.b).await;
    assert_eq!(
        rows(&two.a, "SELECT peer_id FROM peer_inbox").await,
        [linked.a_link]
    );
    assert_eq!(
        rows(&two.b, "SELECT count(*)::text FROM webhook_delivery").await,
        ["0"]
    );
    two.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn the_initiator_cannot_push_events_to_the_installation_whose_events_it_receives() {
    let (two, _) = pair_with_a_hook("admin").await;
    let on_a = rows(&two.a, "SELECT webhook_secret_enc FROM peer_product").await;
    let secret = cipher(&two.a).open(&on_a[0]).unwrap();
    let on_b = rows(&two.b, "SELECT secret FROM webhook_subscription").await;
    assert_eq!(cipher(&two.b).open(&on_b[0]).unwrap(), secret);
    let event = foreign(two.a.instance(), SAVED, Id::ROOT_ACCOUNT);
    let body = serde_json::to_vec(&event).unwrap();
    let signature = sign(&secret, two.b.clock().now().timestamp(), &body).unwrap();
    let pushed = hook(
        &two.b_http,
        &two.a.instance().to_string(),
        &event.id.to_string(),
        Some(&signature),
        &body,
    )
    .await;
    refused(&pushed, 401, ERROR_SIGNATURE);
    assert_eq!(
        rows(&two.b, "SELECT count(*)::text FROM peer_inbox").await,
        ["0"]
    );
    assert_eq!(
        rows(
            &two.b,
            &format!(
                "SELECT count(*)::text FROM keel_outbox WHERE id = '{}' OR instance_id IS NOT NULL",
                event.id
            )
        )
        .await,
        ["0"]
    );
    assert_eq!(
        rows(
            &two.b,
            "SELECT (webhook_secret_enc IS NULL)::text FROM peer_product"
        )
        .await,
        ["true"]
    );
    two.finish().await;
}

#[tokio::test]
async fn an_event_out_of_the_years_of_a_timestamp_is_refused_and_one_that_cannot_enter_the_bus_holds_no_other()
 {
    let (two, linked) = pair_behind_nat("admin", json!([])).await;
    sealed_secret(&two.a, &linked.a_link, SECRET).await;
    for at in ["-5000-01-01T00:00:00Z", "+10000-01-01T00:00:00Z"] {
        let mut event = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
        event["at"] = json!(at);
        let answer = deliver(&two.a, &two.a_http, &event).await;
        refused(&answer, 422, ERROR_EVENT);
        assert_eq!(
            answer.json()["error"]["params"]["reason"],
            json!("at"),
            "{at}"
        );
    }
    assert_eq!(
        rows(&two.a, "SELECT count(*)::text FROM peer_inbox").await,
        ["0"]
    );

    let mut stuck = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
    stuck["at"] = json!("-5000-01-01T00:00:00Z");
    let stuck_id = stuck["id"].as_str().unwrap().to_owned();
    let received = (two.a.clock().now() - keel_sdk::chrono::Duration::minutes(1)).to_rfc3339();
    execute(
        &two.a,
        &format!(
            "INSERT INTO peer_inbox (peer_id, event_id, event_json, received_at, attempts) VALUES ('{}', '{stuck_id}', '{stuck}', '{received}', {})",
            linked.a_link,
            INBOX_MAX_ATTEMPTS - 2
        ),
    )
    .await;
    let valid = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
    let valid_id = valid["id"].as_str().unwrap().to_owned();
    let accepted = deliver(&two.a, &two.a_http, &valid).await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    until_rows(
        &two.a,
        &format!("SELECT instance_id FROM keel_outbox WHERE id = '{valid_id}'"),
        &[two.b.instance().to_string()],
    )
    .await;
    until_rows(
        &two.a,
        &format!(
            "SELECT attempts::text FROM peer_inbox WHERE event_id = '{stuck_id}' AND processed_at IS NOT NULL"
        ),
        &[(INBOX_MAX_ATTEMPTS - 1).to_string()],
    )
    .await;
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT count(*)::text FROM keel_outbox WHERE id = '{stuck_id}'")
        )
        .await,
        ["0"]
    );
    two.finish().await;
}

#[tokio::test]
async fn one_event_id_from_two_links_is_kept_per_link_and_the_second_cannot_take_the_first_on_the_bus()
 {
    let (two, linked) = pair_behind_nat("admin", json!([])).await;
    sealed_secret(&two.a, &linked.a_link, SECRET).await;
    let third = Id::generate();
    let third_link = second_link(&two.a, third).await;
    let original = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
    let id = original["id"].as_str().unwrap().to_owned();
    let mut forged = as_value(&foreign(third, SAVED, Id::ROOT_ACCOUNT));
    forged["id"] = json!(id);
    let first = deliver(&two.a, &two.a_http, &forged).await;
    assert_eq!(
        first.json(),
        json!({ "data": { "eventId": id, "duplicate": false } })
    );
    until_rows(
        &two.a,
        &format!("SELECT instance_id FROM keel_outbox WHERE id = '{id}'"),
        &[third.to_string()],
    )
    .await;
    let second = deliver(&two.a, &two.a_http, &original).await;
    assert_eq!(
        second.json(),
        json!({ "data": { "eventId": id, "duplicate": false } }),
        "the event of another link is not a repeat"
    );
    until_rows(
        &two.a,
        "SELECT peer_id FROM peer_inbox WHERE processed_at IS NOT NULL ORDER BY peer_id",
        &sorted(vec![linked.a_link.clone(), third_link.clone()]),
    )
    .await;
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT instance_id FROM keel_outbox WHERE id = '{id}'")
        )
        .await,
        [third.to_string()]
    );
    let repeated = deliver(&two.a, &two.a_http, &original).await;
    assert_eq!(
        repeated.json(),
        json!({ "data": { "eventId": id, "duplicate": true } })
    );
    two.finish().await;
}

#[tokio::test]
async fn an_inbox_row_of_a_deleted_link_never_enters_the_bus() {
    let (two, linked) = pair_behind_nat("admin", json!([])).await;
    sealed_secret(&two.a, &linked.a_link, SECRET).await;
    let orphan = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
    let orphan_id = orphan["id"].as_str().unwrap().to_owned();
    let received = (two.a.clock().now() - keel_sdk::chrono::Duration::minutes(1)).to_rfc3339();
    execute(
        &two.a,
        &format!(
            "INSERT INTO peer_inbox (peer_id, event_id, event_json, received_at) VALUES ('{}', '{orphan_id}', '{orphan}', '{received}')",
            Id::generate()
        ),
    )
    .await;
    let valid = as_value(&foreign(two.b.instance(), SAVED, Id::ROOT_ACCOUNT));
    let valid_id = valid["id"].as_str().unwrap().to_owned();
    let accepted = deliver(&two.a, &two.a_http, &valid).await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    until(&format!("{valid_id} is processed"), || async {
        processed(&two.a).await == [valid_id.clone()]
    })
    .await;
    assert_eq!(
        rows(
            &two.a,
            &format!(
                "SELECT (processed_at IS NULL)::text || ' ' || (SELECT count(*) FROM keel_outbox WHERE id = '{orphan_id}') FROM peer_inbox WHERE event_id = '{orphan_id}'"
            )
        )
        .await,
        ["true 0"]
    );
    let deleted = two
        .a
        .call(Who::Admin, DELETE, json!({ "id": linked.a_link }))
        .await;
    assert_eq!(deleted.status(), 200, "{deleted:?}");
    assert_eq!(
        rows(
            &two.a,
            &format!(
                "SELECT count(*)::text FROM peer_inbox WHERE peer_id = '{}'",
                linked.a_link
            )
        )
        .await,
        ["0"]
    );
    run_check(&two.a).await;
    assert_eq!(
        rows(&two.a, "SELECT count(*)::text FROM peer_inbox").await,
        ["0"]
    );
    assert_eq!(
        rows(
            &two.a,
            &format!("SELECT count(*)::text FROM keel_outbox WHERE id = '{orphan_id}'")
        )
        .await,
        ["0"]
    );
    two.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn a_pushed_delivery_carries_no_event_from_before_the_pairing_when_the_consumer_lags() {
    let a = with_an_address().await;
    let b = neighbour().await;
    let before = emit(&b, Who::Admin, SAVE).await;
    let two = Two::of(a, b).await;
    give_the_address(&two).await;
    two.pair(
        json!({ "scopes": [], "level": "admin" }),
        json!({ "subscribe": [MASK] }),
    )
    .await;
    assert_eq!(
        rows(
            &two.b,
            &format!(
                "SELECT (s.floor_seq = p.feed_seq AND s.floor_seq >= {})::text FROM webhook_subscription s JOIN peer_product p ON p.id = s.peer_id",
                seq_of(&two.b, &before).await
            )
        )
        .await,
        ["true"]
    );
    let Two {
        a,
        b,
        a_http,
        b_http,
    } = two;
    b_http.stop().await.unwrap();
    until("the webhook consumer reaches the newest event", || async {
        rows(&b, "SELECT seq::text FROM webhook_cursor").await == [newest(&b).await.to_string()]
    })
    .await;
    execute(&b, "UPDATE webhook_cursor SET seq = 0").await;
    let key = b.key().clone();
    let instance = b.instance();
    let database = b.stop().await;
    let b = composed(
        Harness::new()
            .module(&PROBE)
            .key(key)
            .instance(instance)
            .database(database),
    )
    .boot()
    .await;
    let after = emit(&b, Who::Admin, SAVE).await;
    until_rows(
        &a,
        "SELECT event_id FROM peer_inbox WHERE processed_at IS NOT NULL",
        std::slice::from_ref(&after),
    )
    .await;
    let last = seq_of(&b, &after).await;
    until(&format!("the webhook consumer passes {last}"), || async {
        rows(&b, "SELECT seq::text FROM webhook_cursor")
            .await
            .first()
            .and_then(|seq| seq.parse::<u64>().ok())
            .is_some_and(|seq| seq >= last)
    })
    .await;
    assert_eq!(
        rows(&b, "SELECT event_id FROM webhook_delivery").await,
        std::slice::from_ref(&after)
    );
    assert_eq!(
        rows(&a, "SELECT event_id FROM peer_inbox").await,
        [after],
        "{before} was emitted before the pairing"
    );
    a_http.stop().await.unwrap();
    a.finish().await;
    b.finish().await;
}

#[tokio::test]
async fn a_subscription_takes_the_scope_of_its_own_level_beside_another_of_the_same_principal() {
    let (two, linked) = pair_behind_nat("service", json!([])).await;
    let principal = rows(
        &two.b,
        &format!(
            "SELECT principal_id FROM peer_product WHERE id = '{}'",
            linked.b_link
        ),
    )
    .await;
    let sealed = cipher(&two.b).seal(SECRET.as_bytes()).unwrap();
    for (name, peer, level) in [
        ("linked", format!("'{}'", linked.b_link), "'admin'"),
        ("plain", "NULL".to_owned(), "NULL"),
    ] {
        execute(
            &two.b,
            &format!(
                "INSERT INTO webhook_subscription (id, account_id, principal_id, name, url, secret, events, enabled, created_at, created_by, updated_at, peer_id, level) \
                 VALUES ('{}', '{}', '{}', '{name}', 'http://127.0.0.1:9{HOOK_PATH}', '{sealed}', ARRAY['{MASK}'], true, now(), '{name}', now(), {peer}, {level})",
                Id::generate(),
                Id::ROOT_ACCOUNT,
                principal[0],
            ),
        )
        .await;
    }
    let customer = emit(&two.b, Who::A1, SAVE).await;
    let root = emit(&two.b, Who::Admin, SAVE).await;
    let last = seq_of(&two.b, &root).await;
    until(&format!("the webhook consumer passes {last}"), || async {
        rows(&two.b, "SELECT seq::text FROM webhook_cursor")
            .await
            .first()
            .and_then(|seq| seq.parse::<u64>().ok())
            .is_some_and(|seq| seq >= last)
    })
    .await;
    assert_eq!(
        rows(
            &two.b,
            "SELECT s.name || ' ' || d.event_id FROM webhook_delivery d JOIN webhook_subscription s ON s.id = d.subscription_id ORDER BY s.name, d.event_id"
        )
        .await,
        sorted(vec![
            format!("linked {customer}"),
            format!("linked {root}"),
            format!("plain {root}"),
        ])
    );
    two.finish().await;
}

async fn event_stream(http: &TestHttp, credential: (&str, &str)) -> EventStream {
    let mut stream = http.stream(EVENTS_PATH, &[credential]).await;
    assert_eq!(stream.status(), 200, "{}", credential.0);
    assert!(stream.next().await.is_comment(), "{}", credential.0);
    stream
}

async fn invalidated(stream: &mut EventStream) -> (String, String) {
    let frame = stream.next_event().await;
    assert_eq!(frame.event.as_deref(), Some(INVALIDATE), "{frame:?}");
    let event = frame.json();
    (
        event["id"].as_str().unwrap().to_owned(),
        event["instanceId"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn an_event_relayed_from_a_peer_reaches_no_stream_of_another_peer_nor_one_below_admin() {
    let two = Two::of(behind_nat().await, neighbour().await).await;
    let linked = two
        .pair(
            json!({ "scopes": ["auth.token.create"], "level": "admin" }),
            json!({}),
        )
        .await;
    let token = outbound(&two.a, &linked.a_link).await;
    let minted = mint(&two.b_http, &token).await;
    let elsewhere = Id::generate();
    second_link(&two.b, elsewhere).await;
    let peer_bearer = format!("Bearer {token}");
    let minted_bearer = format!("Bearer {minted}");

    let mut admin = event_stream(&two.b_http, (PRINCIPAL_HEADER, Who::Admin.login())).await;
    let mut service = event_stream(&two.b_http, (PRINCIPAL_HEADER, Who::Service.login())).await;
    let mut peer = event_stream(&two.b_http, ("authorization", peer_bearer.as_str())).await;
    let mut derived = event_stream(&two.b_http, ("authorization", minted_bearer.as_str())).await;

    let relayed = as_value(&foreign(elsewhere, SAVED, Id::ROOT_ACCOUNT));
    let relayed_id = relayed["id"].as_str().unwrap().to_owned();
    let accepted = deliver(&two.b, &two.b_http, &relayed).await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);
    until_rows(
        &two.b,
        &format!("SELECT instance_id FROM keel_outbox WHERE id = '{relayed_id}'"),
        &[elsewhere.to_string()],
    )
    .await;
    let here = emit(&two.b, Who::Admin, SAVE).await;
    let hub = two.b.instance().to_string();

    assert_eq!(
        invalidated(&mut admin).await,
        (relayed_id.clone(), elsewhere.to_string())
    );
    assert_eq!(invalidated(&mut admin).await, (here.clone(), hub.clone()));
    for (name, stream) in [
        ("a service principal of the root account", &mut service),
        ("the peer token of another link", &mut peer),
        ("a token the other link minted at admin level", &mut derived),
    ] {
        assert_eq!(
            invalidated(stream).await,
            (here.clone(), hub.clone()),
            "{name} receives the event of this installation and not the relayed one"
        );
        assert!(stream.quiet(QUIET).await, "{name}");
    }
    assert!(admin.quiet(QUIET).await);
    drop((admin, service, peer, derived));
    two.finish().await;
}

#[cfg(feature = "settings")]
#[tokio::test]
async fn an_event_relayed_from_a_peer_reaches_no_user_subscription_below_admin_nor_one_saved_through_a_peer()
 {
    let hub = composed(
        Harness::new()
            .module(&PROBE)
            .module(&keel_system::settings::MODULE),
    )
    .boot()
    .await;
    let two = Two::of(behind_nat().await, hub).await;
    let allowed = two
        .b
        .call(
            Who::Admin,
            "settings.set",
            json!({ "key": keel_system::webhooks::PRIVATE_TARGETS_KEY, "value": true }),
        )
        .await;
    assert_eq!(allowed.status(), 200, "{allowed:?}");
    let linked = two
        .pair(
            json!({ "scopes": ["webhook.subscription.*"], "level": "admin" }),
            json!({}),
        )
        .await;
    let token = outbound(&two.a, &linked.a_link).await;
    let target = "http://127.0.0.1:9/in";
    let saved = crate::peers::call_with(
        &two.b_http,
        &token,
        keel_system::webhooks::SUBSCRIPTION_SET,
        json!({ "name": "through-peer", "url": target, "events": [MASK] }),
    )
    .await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let mine = two
        .b
        .call(
            Who::Admin,
            keel_system::webhooks::SUBSCRIPTION_SET,
            json!({ "name": "admin", "url": target, "events": [MASK] }),
        )
        .await;
    assert_eq!(mine.status(), 200, "{mine:?}");
    let sealed = cipher(&two.b).seal(SECRET.as_bytes()).unwrap();
    let service = two.b.principal(Who::Service).clone();
    assert_eq!(service.account_id, Id::ROOT_ACCOUNT);
    execute(
        &two.b,
        &format!(
            "INSERT INTO webhook_subscription (id, account_id, principal_id, name, url, secret, events, enabled, created_at, created_by, updated_at) \
             VALUES ('{}', '{}', '{}', 'service', '{target}', '{sealed}', ARRAY['{MASK}'], true, now(), 'service', now())",
            Id::generate(),
            Id::ROOT_ACCOUNT,
            service.id,
        ),
    )
    .await;
    let link_principal = crate::peers::link_principal(&two.b, &linked.b_link).await;
    execute(
        &two.b,
        &format!("UPDATE keel_principal SET level = 'admin' WHERE id = '{link_principal}'"),
    )
    .await;
    assert_eq!(
        rows(
            &two.b,
            "SELECT name || ' ' || principal_id || ' ' || local_only::text FROM webhook_subscription ORDER BY name"
        )
        .await,
        [
            format!("admin {} false", two.b.principal(Who::Admin).id),
            format!("service {} false", service.id),
            format!("through-peer {link_principal} true"),
        ]
    );

    let relayed = relay(&two.b)
        .relay(&foreign(Id::generate(), SAVED, Id::ROOT_ACCOUNT))
        .await
        .unwrap()
        .expect("a foreign event enters the outbox");
    let relayed_id = rows(
        &two.b,
        &format!("SELECT id FROM keel_outbox WHERE seq = {relayed}"),
    )
    .await
    .remove(0);
    let here = emit(&two.b, Who::Admin, SAVE).await;
    let last = seq_of(&two.b, &here).await;
    until(&format!("the webhook consumer passes {last}"), || async {
        rows(&two.b, "SELECT seq::text FROM webhook_cursor")
            .await
            .first()
            .and_then(|seq| seq.parse::<u64>().ok())
            .is_some_and(|seq| seq >= last)
    })
    .await;
    assert_eq!(
        rows(
            &two.b,
            "SELECT s.name || ' ' || d.event_id FROM webhook_delivery d JOIN webhook_subscription s ON s.id = d.subscription_id ORDER BY s.name, d.event_id"
        )
        .await,
        sorted(vec![
            format!("admin {relayed_id}"),
            format!("admin {here}"),
            format!("service {here}"),
            format!("through-peer {here}"),
        ])
    );
    two.finish().await;
}
