use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum_server::tls_rustls::RustlsConfig;
use futures::StreamExt;
use keel_client::types::{ActorVia, ErrorCode, Event, Id, Level, PROTOCOL, Page};
use keel_client::{Answer, Client, Credential, Error, OnBehalf, Pin, Trust};
use keel_sdk::sea_orm::{
    ConnectionTrait, DatabaseTransaction, DbBackend, Statement, TransactionTrait,
};
use keel_sdk::system::SecretCipher;
use keel_system::peers::{
    CREATE, ERROR_CUSTOMER, ERROR_QUERY, LOGIN_PREFIX, MAX_EXTERNAL_ID, PAIR_CODE, is_external_id,
    names_peer,
};
use keel_testkit::{Harness, TestHttp, Who};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PASSWORD: &str = "correct-horse-battery-staple";
const UNAVAILABLE: &str =
    r#"{"ok":false,"error":{"code":"unavailable","key":"keel.error.unavailable"}}"#;
const INSTANCE: &str = "01J9A00000000000000000000A";
const GIVE_UP: Duration = Duration::from_secs(5);
const PATIENCE: Duration = Duration::from_secs(30);
const HOLD_ACCOUNTS: &str = "LOCK TABLE keel_account IN SHARE MODE";
const RETRY_WAITS_FOR_THE_HELD_CALL: &str = "SELECT count(*) FROM pg_locks AS retry WHERE retry.locktype = 'advisory' AND NOT retry.granted AND EXISTS (SELECT 1 FROM unnest(pg_blocking_pids(retry.pid)) AS first(pid) WHERE pg_backend_pid() = ANY(pg_blocking_pids(first.pid)))";

#[derive(Debug, Deserialize)]
struct Row {
    id: Id,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Upserted {
    id: Id,
    account_id: Id,
    created: bool,
}

fn client(url: &str, credential: Credential) -> Client {
    Client::builder(url)
        .credential(credential)
        .backoff(Duration::from_millis(10))
        .build()
        .expect("the client is built")
}

async fn token(harness: &Harness, who: Who) -> Credential {
    let reply = harness
        .call(who, "auth.token.create", json!({ "name": "keel-client" }))
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("auth.token.create failed: {:?}", reply.error()));
    Credential::token(data["token"].as_str().expect("the token")).expect("a kat_ token")
}

async fn rows(harness: &Harness, sql: &str) -> Vec<String> {
    harness.database().rows(sql).await
}

async fn accounts(harness: &Harness) -> usize {
    rows(harness, "SELECT id FROM keel_account").await.len()
}

async fn create_account(harness: &Harness) -> Id {
    let reply = harness
        .call(Who::Admin, "keel.account.create", json!({ "kind": "user" }))
        .await;
    let data = reply
        .data()
        .unwrap_or_else(|| panic!("keel.account.create failed: {:?}", reply.error()));
    data["id"].as_str().expect("an id").parse().expect("a ulid")
}

fn subjects(events: &[Event]) -> Vec<Id> {
    events
        .iter()
        .map(|event| event.subject.as_ref().expect("a subject").id)
        .collect()
}

#[tokio::test]
async fn a_call_answers_the_typed_envelope_and_field_errors() {
    let harness = Harness::new()
        .module(&keel_system::auth::MODULE)
        .boot()
        .await;
    let http = harness.http().await;
    let admin = client(&http.url(""), token(&harness, Who::Admin).await);

    let listed: Answer<Page<Row>> = admin
        .call("keel.account.list", json!({}))
        .await
        .expect("the admin lists the accounts");
    let mut listed_ids: Vec<String> = listed
        .data
        .data
        .iter()
        .map(|row| row.id.to_string())
        .collect();
    listed_ids.sort();
    let stored = rows(&harness, "SELECT id FROM keel_account ORDER BY id").await;
    assert_eq!(listed.data.total, stored.len() as u64);
    assert_eq!(listed_ids, stored);
    let principal = listed.principal().expect("meta.principal");
    assert_eq!(principal.login, Who::Admin.login());
    assert_eq!(principal.level, Level::Admin);
    assert!(principal.actor.is_none());
    assert!(!listed.request_id().is_empty());
    assert!(
        listed
            .meta_version
            .as_deref()
            .is_some_and(|version| version.starts_with("m-")),
        "{:?}",
        listed.meta_version
    );
    assert!(!listed.replayed);
    assert!(listed.session.is_none());

    let created: Answer<Row> = admin
        .call("keel.account.create", json!({ "kind": "user" }))
        .await
        .expect("the admin creates an account");
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT kind FROM keel_account WHERE id = '{}'",
                created.data.id
            )
        )
        .await,
        ["user"]
    );

    let misfit = admin
        .call::<Vec<u8>>("keel.account.list", json!({}))
        .await
        .expect_err("a page is not a list of bytes");
    assert!(
        matches!(misfit, Error::Answer { status: 200, .. }),
        "{misfit}"
    );

    let missing = admin
        .call::<Value>("keel.principal.create", json!({ "level": "user" }))
        .await
        .expect_err("login is required");
    assert_eq!(missing.status(), Some(422));
    assert_eq!(missing.code(), Some(ErrorCode::Validation));
    assert_eq!(missing.key(), Some("keel.error.validation"));
    assert_eq!(
        missing
            .field("login")
            .and_then(|field| field.key.as_deref()),
        Some("keel.schema.required"),
        "{missing:?}"
    );
    assert!(
        missing
            .refusal()
            .and_then(|refusal| refusal.request_id())
            .is_some()
    );

    let a1 = harness.principal(Who::A1).clone();
    let taken = admin
        .call::<Value>(
            "keel.principal.create",
            json!({ "login": a1.login, "level": "user", "account": a1.account_id }),
        )
        .await
        .expect_err("the login is taken");
    assert_eq!(taken.code(), Some(ErrorCode::Validation));
    assert_eq!(
        taken.field("login").and_then(|field| field.key.as_deref()),
        Some("keel.principal.error.login_exists"),
        "{taken:?}"
    );

    let unknown = admin
        .call::<Value>("nope.nothing", json!({}))
        .await
        .expect_err("no such function");
    assert_eq!(
        (unknown.status(), unknown.code(), unknown.key()),
        (
            Some(404),
            Some(ErrorCode::NotFound),
            Some("keel.error.function_not_found")
        )
    );
    assert_eq!(
        unknown
            .refusal()
            .and_then(|refusal| refusal.param("function")),
        Some(&json!("nope.nothing"))
    );

    let guest = admin.with_credential(Credential::guest());
    let anonymous = guest
        .call::<Value>("keel.account.list", json!({}))
        .await
        .expect_err("a guest is not let in");
    assert_eq!(
        (anonymous.status(), anonymous.key()),
        (Some(401), Some("keel.error.unauthenticated"))
    );

    let set = harness
        .call(
            Who::Admin,
            "auth.password.set",
            json!({ "principal": a1.id, "password": PASSWORD }),
        )
        .await;
    assert!(set.data().is_some(), "{:?}", set.error());
    let login: Answer<Value> = guest
        .call(
            "auth.login",
            json!({ "login": a1.login, "password": PASSWORD }),
        )
        .await
        .expect("the user logs in");
    assert_eq!(login.data["state"], json!("authenticated"));
    let session = login.session.clone().expect("auth.login sets the session");
    let user = guest.with_credential(session);
    let own: Answer<Page<Row>> = user
        .call("keel.account.list", json!({}))
        .await
        .expect("the session lists");
    assert_eq!(own.data.total, 1);
    assert_eq!(own.data.data[0].id, a1.account_id);
    assert_eq!(
        own.principal().map(|principal| principal.login.as_str()),
        Some(a1.login.as_str())
    );
    let above = user
        .call::<Value>("keel.product.list", json!({}))
        .await
        .expect_err("a user is below the function");
    assert_eq!(
        (above.status(), above.code(), above.key()),
        (
            Some(403),
            Some(ErrorCode::Forbidden),
            Some("keel.error.level")
        )
    );
    user.call::<Value>("auth.logout", json!({}))
        .await
        .expect("the user logs out");
    let ended = user
        .call::<Value>("keel.account.list", json!({}))
        .await
        .expect_err("the session is gone");
    assert_eq!(ended.key(), Some("keel.error.unauthenticated"));

    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[derive(Clone, Copy, Debug)]
enum Fault {
    Lose,
    Drop,
    Unavailable,
}

#[derive(Default)]
struct Script {
    faults: VecDeque<Fault>,
    keys: Vec<Option<String>>,
}

struct Proxy {
    addr: SocketAddr,
    script: Arc<Mutex<Script>>,
    task: tokio::task::JoinHandle<()>,
}

impl Proxy {
    async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
        let addr = listener.local_addr().expect("the bound address");
        let script = Arc::new(Mutex::new(Script::default()));
        let shared = script.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let script = shared.clone();
                tokio::spawn(async move {
                    let _ = relay(stream, target, script).await;
                });
            }
        });
        Self { addr, script, task }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn arm(&self, faults: &[Fault]) {
        let mut script = self.script.lock().unwrap_or_else(PoisonError::into_inner);
        script.faults = faults.iter().copied().collect();
        script.keys.clear();
    }

    fn keys(&self) -> Vec<Option<String>> {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys
            .clone()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let head = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        raw.extend_from_slice(&buffer[..read]);
    };
    let length = header_of(&raw[..head], "content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    while raw.len() < head + length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        raw.extend_from_slice(&buffer[..read]);
    }
    Ok(raw)
}

fn header_of(head: &[u8], name: &str) -> Option<String> {
    String::from_utf8_lossy(head).lines().find_map(|line| {
        let (field, value) = line.split_once(':')?;
        field
            .trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

fn closing(request: &[u8]) -> Vec<u8> {
    let head = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a whole head");
    let text = String::from_utf8_lossy(&request[..head]);
    let mut rewritten: String = text
        .split("\r\n")
        .filter(|line| {
            !line
                .split_once(':')
                .is_some_and(|(field, _)| field.trim().eq_ignore_ascii_case("connection"))
        })
        .map(|line| format!("{line}\r\n"))
        .collect();
    rewritten.push_str("connection: close\r\n\r\n");
    let mut bytes = rewritten.into_bytes();
    bytes.extend_from_slice(&request[head + 4..]);
    bytes
}

async fn relay(
    mut client: TcpStream,
    target: SocketAddr,
    script: Arc<Mutex<Script>>,
) -> io::Result<()> {
    let request = read_request(&mut client).await?;
    let fault = {
        let mut script = script.lock().unwrap_or_else(PoisonError::into_inner);
        script.keys.push(header_of(&request, "idempotency-key"));
        script.faults.pop_front()
    };
    match fault {
        Some(Fault::Drop) => return Ok(()),
        Some(Fault::Unavailable) => {
            let answer = format!(
                "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{UNAVAILABLE}",
                UNAVAILABLE.len()
            );
            client.write_all(answer.as_bytes()).await?;
            return client.shutdown().await;
        }
        Some(Fault::Lose) | None => {}
    }
    let mut upstream = TcpStream::connect(target).await?;
    upstream.write_all(&closing(&request)).await?;
    let mut answer = Vec::new();
    upstream.read_to_end(&mut answer).await?;
    if matches!(fault, Some(Fault::Lose)) {
        return Ok(());
    }
    client.write_all(&answer).await?;
    client.shutdown().await
}

async fn hold_accounts(harness: &Harness) -> DatabaseTransaction {
    let gate = harness.db().begin().await.expect("a transaction");
    gate.execute_unprepared(HOLD_ACCOUNTS)
        .await
        .expect("the account table is held");
    gate
}

async fn wait_retry_behind_held_call(gate: &DatabaseTransaction) {
    let waiting = Statement::from_string(DbBackend::Postgres, RETRY_WAITS_FOR_THE_HELD_CALL);
    tokio::time::timeout(PATIENCE, async {
        loop {
            let row = gate
                .query_one(waiting.clone())
                .await
                .expect("the locks are read")
                .expect("a count");
            if row.try_get_by_index::<i64>(0).expect("a count") > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the retry waits for the claim of the call the table holds");
}

#[tokio::test]
async fn a_call_with_a_key_is_retried_and_one_without_is_not() {
    let harness = Harness::new()
        .module(&keel_system::auth::MODULE)
        .boot()
        .await;
    let http = harness.http().await;
    let proxy = Proxy::start(http.addr()).await;
    let credential = token(&harness, Who::Admin).await;
    let admin = client(&proxy.url(), credential.clone());
    let user = json!({ "kind": "user" });
    let before = accounts(&harness).await;

    proxy.arm(&[Fault::Lose]);
    let lost: Answer<Row> = admin
        .call("keel.account.create", &user)
        .idempotency_key("client-lost")
        .await
        .expect("the retry answers the committed call");
    assert!(
        lost.replayed,
        "the retry is answered from the idempotency store"
    );
    let key = Some("client-lost".to_owned());
    assert_eq!(proxy.keys(), [key.clone(), key]);
    assert_eq!(accounts(&harness).await, before + 1);
    assert_eq!(
        rows(
            &harness,
            &format!("SELECT id FROM keel_account WHERE id = '{}'", lost.data.id)
        )
        .await,
        [lost.data.id.to_string()]
    );

    proxy.arm(&[]);
    let impatient = Client::builder(&proxy.url())
        .credential(credential)
        .timeout(GIVE_UP)
        .backoff(Duration::from_millis(10))
        .build()
        .expect("the client is built");
    let gate = hold_accounts(&harness).await;
    let held = tokio::spawn(async move {
        impatient
            .call::<Row>("keel.account.create", json!({ "kind": "user" }))
            .idempotency_key("client-timeout")
            .await
    });
    wait_retry_behind_held_call(&gate).await;
    gate.commit().await.expect("the account table is released");
    let timed_out = held
        .await
        .expect("the call runs to its end")
        .expect("the retry answers the first attempt once it commits");
    assert!(
        timed_out.replayed,
        "the retry waits for the first attempt and is answered from the idempotency store"
    );
    let key = Some("client-timeout".to_owned());
    assert_eq!(proxy.keys(), [key.clone(), key]);
    assert_eq!(accounts(&harness).await, before + 2);
    assert_eq!(
        rows(
            &harness,
            &format!(
                "SELECT id FROM keel_account WHERE id = '{}'",
                timed_out.data.id
            )
        )
        .await,
        [timed_out.data.id.to_string()]
    );

    proxy.arm(&[Fault::Unavailable]);
    let unavailable: Answer<Row> = admin
        .call("keel.account.create", &user)
        .idempotency_key("client-unavailable")
        .await
        .expect("the retry after a 503 runs the call");
    assert!(!unavailable.replayed);
    assert_eq!(proxy.keys().len(), 2);
    assert_eq!(accounts(&harness).await, before + 3);

    proxy.arm(&[Fault::Drop, Fault::Drop, Fault::Drop]);
    let exhausted = admin
        .call::<Row>("keel.account.create", &user)
        .idempotency_key("client-exhausted")
        .await
        .expect_err("every attempt fails");
    assert!(
        matches!(exhausted, Error::Transport { attempts: 3, .. }),
        "{exhausted}"
    );
    assert_eq!(proxy.keys().len(), 3);
    assert_eq!(accounts(&harness).await, before + 3);

    proxy.arm(&[]);
    let refused = admin
        .call::<Row>("keel.account.create", json!({ "kind": "Not A Name!" }))
        .idempotency_key("client-refused")
        .await
        .expect_err("the kind is not a name");
    assert_eq!(refused.status(), Some(422));
    assert_eq!(proxy.keys().len(), 1, "a refusal below 500 is not retried");

    proxy.arm(&[Fault::Lose]);
    let unkeyed = admin
        .call::<Row>("keel.account.create", &user)
        .await
        .expect_err("the answer is lost and not asked again");
    assert!(
        matches!(unkeyed, Error::Transport { attempts: 1, .. }),
        "{unkeyed}"
    );
    assert_eq!(proxy.keys(), [None]);
    assert_eq!(
        accounts(&harness).await,
        before + 4,
        "the call committed once and was not repeated"
    );

    proxy.arm(&[Fault::Unavailable]);
    let unkeyed = admin
        .call::<Row>("keel.account.create", &user)
        .await
        .expect_err("a 503 without a key is the answer");
    assert_eq!(
        (unkeyed.status(), unkeyed.code(), unkeyed.key()),
        (
            Some(503),
            Some(ErrorCode::Unavailable),
            Some("keel.error.unavailable")
        )
    );
    assert_eq!(proxy.keys(), [None]);
    assert_eq!(accounts(&harness).await, before + 4);

    drop(proxy);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

struct Two {
    a: Harness,
    b: Harness,
    a_http: TestHttp,
    b_http: TestHttp,
}

async fn peer_installation() -> Harness {
    Harness::new()
        .module(&keel_system::auth::MODULE)
        .module(&keel_system::cron::MODULE)
        .module(&keel_system::notify::MODULE)
        .module(&keel_system::webhooks::MODULE)
        .module(&keel_system::peers::MODULE)
        .boot()
        .await
}

impl Two {
    async fn start() -> Self {
        let a = peer_installation().await;
        let b = peer_installation().await;
        let a_http = a.http().await;
        let b_http = b.http().await;
        Self {
            a,
            b,
            a_http,
            b_http,
        }
    }

    async fn pair(&self, b_grants: Value, a_grants: Value) -> Credential {
        let issued = self.b.call(Who::Admin, PAIR_CODE, b_grants).await;
        let code = issued
            .data()
            .unwrap_or_else(|| panic!("{PAIR_CODE} failed: {:?}", issued.error()))["code"]
            .clone();
        let mut params = a_grants;
        params["url"] = json!(self.b_http.url(""));
        params["code"] = code;
        let created = self.a.call(Who::Admin, CREATE, params).await;
        let link = created
            .data()
            .unwrap_or_else(|| panic!("{CREATE} failed: {:?}", created.error()))["id"]
            .as_str()
            .expect("the link id")
            .to_owned();
        let sealed = rows(
            &self.a,
            &format!("SELECT outbound_token_enc FROM peer_product WHERE id = '{link}'"),
        )
        .await;
        let cipher = self
            .a
            .registry()
            .services()
            .get::<dyn SecretCipher>()
            .expect("the kernel seals with the data key");
        let token = String::from_utf8(cipher.open(&sealed[0]).expect("the data key opens it"))
            .expect("the token is text");
        Credential::peer(token).expect("a kpt_ token")
    }

    fn b_url(&self) -> String {
        self.b_http.url("")
    }

    async fn finish(self) {
        self.a_http.stop().await.expect("a stops serving");
        self.b_http.stop().await.expect("b stops serving");
        self.a.finish().await;
        self.b.finish().await;
    }
}

#[tokio::test]
async fn on_behalf_reaches_the_customer() {
    let two = Two::start().await;
    let token = two
        .pair(
            json!({ "scopes": ["keel.account.*", "keel.principal.*"], "level": "admin", "onBehalf": true }),
            json!({}),
        )
        .await;
    let origin = format!("bill:{}", two.a.instance());
    let peer = client(&two.b_url(), token);

    let upserted: Answer<Upserted> = peer
        .call(
            "keel.principal.upsert",
            json!({
                "origin": origin, "externalId": "client-42", "login": "alice", "level": "user",
                "account": { "origin": origin, "externalId": "acc-42", "kind": "user" },
            }),
        )
        .await
        .expect("the link places its customer");
    assert!(upserted.data.created);
    let link = upserted.principal().expect("the link calls").clone();
    assert!(link.login.starts_with(LOGIN_PREFIX), "{}", link.login);
    assert!(link.actor.is_none());

    let customer = peer.with_on_behalf(Some(
        OnBehalf::new(&origin, "client-42").expect("a well-formed customer"),
    ));
    let listed: Answer<Page<Row>> = customer
        .call("keel.account.list", json!({}))
        .await
        .expect("the customer lists its accounts");
    assert_eq!(listed.data.total, 1);
    assert_eq!(listed.data.data[0].id, upserted.data.account_id);
    let principal = listed.principal().expect("meta.principal");
    assert_eq!(principal.id, upserted.data.id);
    assert_eq!(principal.login, "alice");
    assert_eq!(principal.level, Level::User);
    assert_eq!(principal.account, upserted.data.account_id);
    let actor = principal.actor.as_ref().expect("the link acts");
    assert_eq!(actor.id, link.id);
    assert_eq!(actor.login, link.login);
    assert_eq!(actor.via, ActorVia::Peer);

    let above = customer
        .call::<Value>("keel.account.create", json!({ "kind": "user" }))
        .await
        .expect_err("the customer's level applies");
    assert_eq!(
        (above.status(), above.key()),
        (Some(403), Some("keel.error.level"))
    );

    let once: Answer<Page<Row>> = peer
        .call("keel.account.list", json!({}))
        .on_behalf(OnBehalf::new(&origin, "client-42").expect("a well-formed customer"))
        .await
        .expect("one call on behalf");
    assert_eq!(
        once.principal().map(|principal| principal.login.as_str()),
        Some("alice")
    );
    let itself: Answer<Page<Row>> = peer
        .call("keel.account.list", json!({}))
        .await
        .expect("the link as itself");
    assert_eq!(
        itself.principal().map(|principal| principal.id),
        Some(link.id)
    );
    assert!(itself.data.total > 1);

    let stranger = peer
        .call::<Value>("keel.account.list", json!({}))
        .on_behalf(OnBehalf::new(&origin, "client-43").expect("a well-formed customer"))
        .await
        .expect_err("no such customer");
    assert_eq!(
        (stranger.status(), stranger.key()),
        (Some(404), Some(ERROR_CUSTOMER))
    );
    assert_eq!(
        stranger
            .refusal()
            .and_then(|refusal| refusal.param("externalId")),
        Some(&json!("client-43"))
    );

    two.finish().await;
}

#[tokio::test]
async fn the_pull_feed_streams_the_events_after_its_cursor_and_ends_when_caught_up() {
    let two = Two::start().await;
    let token = two
        .pair(
            json!({ "scopes": [], "level": "admin" }),
            json!({ "subscribe": ["keel.account.*"] }),
        )
        .await;
    let peer = client(&two.b_url(), token);

    let handshake = peer.instance().await.expect("the handshake");
    assert_eq!(handshake.instance_id, two.b.instance());
    assert_eq!(handshake.protocol, PROTOCOL);
    assert!(
        handshake.features.iter().any(|feature| feature == "peers"),
        "{handshake:?}"
    );

    let mut created = Vec::new();
    for _ in 0..5 {
        created.push(create_account(&two.b).await);
    }
    let mut feed = peer.feed(0).limit(2);
    let mut seen = Vec::new();
    while let Some(event) = feed.next().await {
        seen.push(event.expect("an event of the feed"));
    }
    assert_eq!(subjects(&seen), created);
    assert!(
        seen.iter()
            .all(|event| event.kind == "keel.account.created"
                && event.instance_id == two.b.instance()),
        "{seen:?}"
    );
    let cursor = feed.cursor();
    assert!(cursor >= seen.last().expect("five events").seq);
    assert!(
        feed.next().await.is_none(),
        "a finished feed stays finished"
    );

    let sixth = create_account(&two.b).await;
    let resumed: Vec<Event> = peer
        .feed(cursor)
        .map(|event| event.expect("an event of the feed"))
        .collect()
        .await;
    assert_eq!(subjects(&resumed), [sixth]);

    let acting = peer.with_on_behalf(Some(
        OnBehalf::new(format!("bill:{}", two.a.instance()), "nobody")
            .expect("a well-formed customer"),
    ));
    assert_eq!(
        acting.instance().await.expect("the handshake").instance_id,
        two.b.instance()
    );
    let whole: Vec<Event> = acting
        .feed(0)
        .map(|event| event.expect("the feed goes as the link itself"))
        .collect()
        .await;
    let mut every = created.clone();
    every.push(sixth);
    assert_eq!(subjects(&whole), every);

    let mut filtered = peer.feed(0).types(["keel.principal.*"]);
    assert!(filtered.next().await.is_none());
    assert!(
        filtered.cursor() >= resumed[0].seq,
        "the cursor passes the events the masks leave out"
    );

    let guest = peer
        .with_credential(Credential::guest())
        .feed(0)
        .next()
        .await
        .expect("an item")
        .expect_err("the feed answers a peer token alone");
    assert_eq!(
        (guest.status(), guest.key()),
        (Some(401), Some("keel.error.unauthenticated"))
    );
    let mut malformed = peer.feed(0).limit(0);
    let refused = malformed
        .next()
        .await
        .expect("an item")
        .expect_err("a limit of zero is refused");
    assert_eq!(
        refused
            .field("limit")
            .and_then(|field| field.key.as_deref()),
        Some(ERROR_QUERY)
    );
    assert!(malformed.next().await.is_none(), "an error ends the feed");

    two.finish().await;
}

async fn serve_tls(
    router: Router,
    certified: &rcgen::CertifiedKey<rcgen::KeyPair>,
) -> (SocketAddr, axum_server::Handle<SocketAddr>) {
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the protocol versions")
    .with_no_client_auth()
    .with_single_cert(vec![certified.cert.der().clone()], key)
    .expect("a certificate and its key");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let addr = listener.local_addr().expect("the bound address");
    let listener = listener.into_std().expect("a std listener");
    let handle = axum_server::Handle::new();
    let server =
        axum_server::from_tcp_rustls(listener, RustlsConfig::from_config(Arc::new(config)))
            .expect("the TLS server")
            .handle(handle.clone());
    tokio::spawn(async move {
        let _ = server
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await;
    });
    (addr, handle)
}

#[tokio::test]
async fn a_pinned_certificate_is_trusted_whatever_its_names_and_another_or_the_system_roots_are_refused()
 {
    let harness = Harness::new().boot().await;
    let served = rcgen::generate_simple_self_signed(vec!["keel.test".to_owned()])
        .expect("the served certificate");
    let other = rcgen::generate_simple_self_signed(vec!["keel.test".to_owned()])
        .expect("another certificate");
    let (addr, handle) = serve_tls(harness.router(), &served).await;
    let url = format!("https://{addr}");
    let trusting = |trust: Trust| {
        Client::builder(&url)
            .trust(trust)
            .build()
            .expect("the client is built")
    };

    let pinned = trusting(Trust::Pinned(vec![Pin::of(served.cert.der())]));
    assert_eq!(
        pinned
            .instance()
            .await
            .expect("the pinned handshake")
            .instance_id,
        harness.instance()
    );
    let printed: Pin = Pin::of(served.cert.der())
        .to_string()
        .parse()
        .expect("the printed fingerprint");
    let from_print = trusting(Trust::Pinned(vec![printed]));
    from_print
        .instance()
        .await
        .expect("the handshake pinned by the fingerprint");
    let from_pem = trusting(Trust::Pinned(
        Pin::from_pem(served.cert.pem().as_bytes()).expect("the PEM"),
    ));
    let refused = from_pem
        .call::<Value>("keel.account.list", json!({}))
        .await
        .expect_err("a guest is not let in");
    assert_eq!(refused.key(), Some("keel.error.unauthenticated"));
    let shouted = Client::builder(&format!("HTTPS://{addr}/"))
        .trust(Trust::Pinned(vec![Pin::of(served.cert.der())]))
        .build()
        .expect("the client is built");
    assert_eq!(shouted.base(), url);
    assert_eq!(
        shouted
            .instance()
            .await
            .expect("the pinned handshake under an uppercase scheme")
            .instance_id,
        harness.instance()
    );
    assert!(matches!(
        Client::builder(&format!("http://{addr}"))
            .trust(Trust::Pinned(vec![Pin::of(served.cert.der())]))
            .build(),
        Err(Error::Invalid(_))
    ));

    let foreign = trusting(Trust::Pinned(vec![Pin::of(other.cert.der())]));
    let error = foreign
        .instance()
        .await
        .expect_err("another certificate is not trusted");
    assert!(error.is_transport(), "{error}");
    match Client::builder(&url).trust(Trust::System).build() {
        Ok(system) => {
            let error = system
                .instance()
                .await
                .expect_err("a self-signed certificate is not among the system roots");
            assert!(error.is_transport(), "{error}");
        }
        Err(error) => assert!(matches!(error, Error::Invalid(_)), "{error}"),
    }
    assert!(matches!(
        Client::builder(&url)
            .trust(Trust::Pinned(Vec::new()))
            .build(),
        Err(Error::Invalid(_))
    ));

    handle.shutdown();
    harness.finish().await;
}

type Hits = Arc<Mutex<BTreeMap<String, usize>>>;

async fn canned(uri: Uri, hits: Hits) -> Response {
    let path = uri.path().to_owned();
    *hits
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(path.clone())
        .or_default() += 1;
    match path.as_str() {
        "/api/v1/call/moved" => (
            StatusCode::FOUND,
            [(header::LOCATION, "https://elsewhere.test/")],
        )
            .into_response(),
        "/api/v1/call/big" => (StatusCode::OK, " ".repeat(4096)).into_response(),
        "/api/v1/call/broken" => {
            (StatusCode::INTERNAL_SERVER_ERROR, "<html>oops</html>").into_response()
        }
        "/api/v1/call/mismatch" => (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"ok":false,"error":{"code":"validation","key":"keel.error.validation"}}"#,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::test]
async fn a_redirect_an_oversized_body_or_an_answer_outside_the_envelope_is_an_answer_error() {
    let hits = Hits::default();
    let shared = hits.clone();
    let router = Router::new().fallback(move |uri: Uri| canned(uri, shared.clone()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a free port");
    let addr = listener.local_addr().expect("the bound address");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let canned = Client::builder(&format!("http://{addr}"))
        .attempts(4)
        .backoff(Duration::from_millis(1))
        .answer_limit(1024)
        .build()
        .expect("the client is built");
    let count = |path: &str| {
        hits.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(path)
            .copied()
            .unwrap_or_default()
    };

    for (func, status, reason, tries) in [
        ("moved", 302, "redirect", 1),
        ("big", 200, "larger than 1024 bytes", 1),
        ("mismatch", 403, "error.code validation", 1),
        ("broken", 500, "not an envelope", 4),
    ] {
        let error = canned
            .call::<Value>(func, json!({}))
            .idempotency_key(&format!("canned-{func}"))
            .await
            .expect_err("outside the contract");
        match &error {
            Error::Answer {
                status: answered,
                reason: told,
                ..
            } => {
                assert_eq!(*answered, status, "{error}");
                assert!(told.contains(reason), "{error}");
            }
            other => panic!("{func}: {other}"),
        }
        assert_eq!(count(&format!("/api/v1/call/{func}")), tries, "{func}");
    }
    let raw = canned
        .send(keel_client::Request::post("/api/v1/call/moved", &json!({})))
        .await
        .expect("a raw request answers what the server said");
    assert_eq!(raw.status, 302);
    assert_eq!(
        raw.header(header::LOCATION),
        Some("https://elsewhere.test/")
    );
    assert_eq!(count("/api/v1/call/moved"), 2);
    let unkeyed = canned
        .call::<Value>("broken", json!({}))
        .await
        .expect_err("outside the contract");
    assert_eq!(unkeyed.status(), Some(500));
    assert_eq!(count("/api/v1/call/broken"), 5);
    let invalid = canned
        .call::<Value>("../boot", json!({}))
        .await
        .expect_err("not a function name");
    assert!(matches!(invalid, Error::Invalid(_)), "{invalid}");
    server.abort();
}

#[test]
fn a_customer_is_built_exactly_when_the_installation_reads_its_header() {
    let origin = format!("bill:{INSTANCE}");
    let mut origins = vec![
        origin.clone(),
        format!("crm_2:{INSTANCE}"),
        format!("{}:{INSTANCE}", "b".repeat(64)),
        format!("{}:{INSTANCE}", "b".repeat(65)),
        String::new(),
        "bill".to_owned(),
        "bill:01J9A".to_owned(),
        INSTANCE.to_owned(),
        format!("bill:{}", INSTANCE.to_lowercase()),
        format!("bill:{INSTANCE}:x"),
    ];
    let mut external_ids = vec!["x".repeat(MAX_EXTERNAL_ID), "x".repeat(MAX_EXTERNAL_ID + 1)];
    for byte in 0u8..=127 {
        let character = char::from(byte);
        origins.push(format!("{character}ill:{INSTANCE}"));
        origins.push(format!("b{character}ll:{INSTANCE}"));
        external_ids.push(format!("client{character}42"));
    }
    external_ids.extend([String::new(), "client\u{e9}".to_owned()]);
    for candidate in &origins {
        assert_eq!(
            OnBehalf::new(candidate.as_str(), "client-42").is_ok(),
            names_peer(candidate, INSTANCE),
            "{candidate:?}"
        );
    }
    for candidate in &external_ids {
        assert_eq!(
            OnBehalf::new(origin.as_str(), candidate.as_str()).is_ok(),
            is_external_id(candidate),
            "{candidate:?}"
        );
    }
    assert_eq!(keel_client::types::EXTERNAL_MAX_LEN, MAX_EXTERNAL_ID);
}

#[test]
fn the_paths_and_the_cookie_are_the_ones_the_installation_serves() {
    assert_eq!(keel_client::FEED_PATH, keel_system::peers::FEED_PATH);
    assert_eq!(keel_client::SESSION_COOKIE, keel_testkit::SESSION_COOKIE);
    assert_eq!(
        keel_client::TOKEN_PREFIX,
        keel_system::auth::API_TOKEN_PREFIX
    );
    assert_eq!(
        keel_client::PEER_PREFIX,
        keel_system::auth::PEER_TOKEN_PREFIX
    );
    assert_eq!(keel_client::SESSION_PREFIX, keel_system::auth::TOKEN_PREFIX);
}
