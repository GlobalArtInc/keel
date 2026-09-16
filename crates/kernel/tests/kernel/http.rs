use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, ETAG, HOST, IF_NONE_MATCH, ORIGIN,
};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use keel_kernel::dispatcher::{CallOptions, Dispatcher};
use keel_kernel::http::{
    IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY, META_VERSION, REQUEST_ID, SESSION_COOKIE,
};
use keel_kernel::{Composition, Http, HttpError, HttpOptions, Kernel, Readiness, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Authenticated, Authenticator, CallExt, Client, Credential, SessionChange};
use keel_testkit::TestDatabase;
use keel_types::{ErrorCode, Scope, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

const ROOT: &str = "00000000000000000000000000";
const RESELLER: &str = "01J8A0000000000000000000A1";
const CUSTOMER: &str = "01J8A0000000000000000000A2";
const FORWARDED_FOR: &str = "x-forwarded-for";
const FORWARDED_HOST: &str = "x-forwarded-host";

const EN: &str = r#"{ "probe": { "greeting": "Hello", "only_en": "English only" } }"#;
const RU: &str = r#"{ "probe": { "greeting": "ru:Hello" } }"#;
const KERNEL_RU: &str = include_str!("../../i18n/ru.json");

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pong {
    login: String,
    level: Level,
    lang: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Fail {
    code: ErrorCode,
}

struct TestAuthenticator;

#[async_trait]
impl Authenticator for TestAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        let (token, session) = match credential {
            Credential::Bearer(secret) => (secret.expose(), None),
            Credential::Session(secret) => {
                (secret.expose(), Some(format!("row-of-{}", secret.expose())))
            }
        };
        let principal = match token.split_once(':') {
            Some(("login", login)) => stored(db, login).await?,
            Some((level, account)) => {
                let level: Level = level
                    .parse()
                    .map_err(|_| KeelError::internal("probe.error.level"))?;
                let account: Id = account
                    .parse()
                    .map_err(|_| KeelError::internal("probe.error.account"))?;
                Some(principal(level, account))
            }
            None if token == "boom" => return Err(KeelError::internal("probe.error.boom")),
            None => None,
        };
        Ok(principal.map(|principal| Authenticated { principal, session }))
    }
}

async fn stored(db: &Db, login: &str) -> Result<Option<Principal>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, account_id, level, role, lang FROM keel_principal WHERE login = $1",
            [login.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let level: String = row.try_get("", "level")?;
    let id: String = row.try_get("", "id")?;
    let account: String = row.try_get("", "account_id")?;
    Ok(Some(Principal {
        id: id
            .parse()
            .map_err(|_| KeelError::internal("probe.error.id"))?,
        login: login.to_owned(),
        level: level
            .parse()
            .map_err(|_| KeelError::internal("probe.error.level"))?,
        account_id: account
            .parse()
            .map_err(|_| KeelError::internal("probe.error.account"))?,
        role: row.try_get("", "role")?,
        actor: None,
        scope: Scope::Full,
        via: Via::Http,
        external_id: None,
        origin: None,
        lang: row.try_get("", "lang")?,
        grants: BTreeMap::new(),
    }))
}

fn principal(level: Level, account: Id) -> Principal {
    let rank = Level::ALL
        .iter()
        .position(|candidate| *candidate == level)
        .unwrap_or_default() as u64;
    let id = ulid::Ulid::from_parts(rank + 1, ulid::Ulid::from(account).random());
    Principal {
        id: Id::from(id),
        login: format!("{level}-{account}"),
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

static PROBE: ModuleDef = ModuleDef {
    id: "probe",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Pong>("probe.ping")
            .kind(Kind::Get)
            .level(Level::Guest)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Pong {
                    login: call.principal().login.clone(),
                    level: call.principal().level,
                    lang: call.principal().lang.clone(),
                })
            }),
    )
    .function(
        function::<Fail, Nothing>("probe.fail")
            .kind(Kind::Get)
            .level(Level::Guest)
            .unowned()
            .handler(|call: Call<Fail>| async move {
                Err(KeelError::new(call.params().code).param("raised", true))
            }),
    )
    .function(
        function::<Nothing, Nothing>("probe.admin")
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) }),
    )
    .function(
        function::<Nothing, Seen>("probe.client")
            .kind(Kind::Get)
            .level(Level::Guest)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Seen {
                    client: call.client().cloned().map(ClientSeen::from),
                })
            }),
    )
    .function(
        function::<Cookie, Nothing>("probe.cookie")
            .kind(Kind::Action)
            .level(Level::Guest)
            .unowned()
            .handler(|call: Call<Cookie>| async move {
                match call.params().change.as_str() {
                    "set" => call.session(SessionChange::Set(Secret::new("ks_probe"))),
                    "clear" => call.session(SessionChange::Clear),
                    _ => {}
                }
                if call.params().fail {
                    return Err(KeelError::conflict("probe.error.after_cookie"));
                }
                Ok(Nothing {})
            }),
    )
    .i18n("en", EN)
    .i18n("ru", RU)
    .contribute::<Arc<dyn Authenticator>>(Arc::new(TestAuthenticator));
    Ok(())
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Seen {
    client: Option<ClientSeen>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ClientSeen {
    ip: Option<String>,
    user_agent: Option<String>,
    session: Option<String>,
}

impl From<Client> for ClientSeen {
    fn from(client: Client) -> Self {
        Self {
            ip: client.ip.map(|ip| ip.to_string()),
            user_agent: client.user_agent,
            session: client.session,
        }
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Cookie {
    change: String,
    #[serde(default)]
    fail: bool,
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Answer {
    fn header(&self, name: impl axum::http::header::AsHeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn request_id(&self) -> &str {
        self.header(REQUEST_ID)
            .expect("X-Keel-Request-Id is present")
    }

    fn code(&self) -> ErrorCode {
        assert_eq!(self.body["ok"], json!(false), "{}", self.body);
        serde_json::from_value(self.body["error"]["code"].clone()).expect("an error code")
    }

    fn key(&self) -> &str {
        self.body["error"]["key"].as_str().expect("an error key")
    }

    fn data(&self) -> &Value {
        assert_eq!(self.body["ok"], json!(true), "{}", self.body);
        &self.body["data"]
    }
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Arc<Dispatcher>,
    readiness: Arc<Readiness>,
    router: Router,
}

async fn bench() -> Bench {
    bench_with(Composition::new().module(&PROBE), HttpOptions::default()).await
}

async fn bench_with(composition: Composition, options: HttpOptions) -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(composition, &storage)
        .await
        .expect("the kernel boots");
    for (id, kind, parent, path) in [
        (RESELLER, "reseller", ROOT, format!("/{RESELLER}/")),
        (
            CUSTOMER,
            "user",
            RESELLER,
            format!("/{RESELLER}/{CUSTOMER}/"),
        ),
    ] {
        storage
            .db()
            .execute_unprepared(&format!(
                "INSERT INTO keel_account (id, kind, parent_id, path, created_at, updated_at) VALUES ('{id}', '{kind}', '{parent}', '{path}', now(), now())"
            ))
            .await
            .expect("seed an account");
    }
    let dispatcher = Arc::new(
        Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles"),
    );
    let readiness = Readiness::new();
    readiness.migrated();
    let http = Http::new(storage.db().clone(), readiness.clone(), options);
    http.attach(dispatcher.clone())
        .expect("the kernel attaches once");
    let router = http.router();
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
        readiness,
        router,
    }
}

impl Bench {
    async fn send(&self, request: Request<Body>) -> Answer {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "{status}: the body is not JSON: {error}: {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        Answer {
            status,
            headers,
            body,
        }
    }

    async fn get(&self, path: &str, auth: Option<&str>) -> Answer {
        let mut request = Request::builder().method(Method::GET).uri(path);
        if let Some(token) = auth {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        self.send(request.body(Body::empty()).expect("a request"))
            .await
    }

    async fn call(&self, func: &str, auth: Option<&str>, params: Value) -> Answer {
        self.post(func, auth, &[], params).await
    }

    async fn post(
        &self,
        func: &str,
        auth: Option<&str>,
        headers: &[(&str, &str)],
        params: Value,
    ) -> Answer {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/call/{func}"))
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = auth {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(
            request
                .body(Body::from(params.to_string()))
                .expect("a request"),
        )
        .await
    }

    async fn as_owner(&self, func: &str, params: Value) -> Value {
        let reply = self
            .dispatcher
            .call(
                principal(Level::Owner, Id::ROOT_ACCOUNT),
                func,
                params,
                CallOptions::default(),
            )
            .await;
        reply
            .data()
            .cloned()
            .unwrap_or_else(|| panic!("{func} as owner: {:?}", reply.error()))
    }

    async fn finish(self) {
        drop(self.router);
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn user() -> String {
    format!("user:{CUSTOMER}")
}

fn reseller() -> String {
    format!("reseller:{RESELLER}")
}

fn admin() -> String {
    format!("admin:{ROOT}")
}

fn names(descriptors: &Value) -> Vec<&str> {
    descriptors
        .as_array()
        .expect("a list of descriptors")
        .iter()
        .map(|descriptor| descriptor["name"].as_str().expect("a name"))
        .collect()
}

#[tokio::test]
async fn healthz_answers_at_once_and_readyz_follows_migrations_and_draining() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let readiness = Readiness::new();
    let http = Http::new(
        storage.db().clone(),
        readiness.clone(),
        HttpOptions::default(),
    );
    let served = http
        .clone()
        .bind_local()
        .await
        .expect("the listener binds before the kernel boots");
    let addr = served.addr();
    let probe = |path: &'static str| async move {
        let (status, head, body) = raw(
            addr,
            &format!("GET {path} HTTP/1.1\r\nHost: keel.test\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            head.to_ascii_lowercase().contains("x-keel-request-id:"),
            "{path}: {head}"
        );
        let body: Value = serde_json::from_str(&body).unwrap_or_else(|error| {
            panic!("{path}: {status}: the body is not JSON: {error}: {body:?}")
        });
        (status, body)
    };
    let (status, body) = probe("/healthz").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({ "status": "ok" }));
    let (status, body) = probe("/readyz").await;
    assert_eq!(status, 503);
    assert_eq!(body, json!({ "status": "migrating" }));
    for path in [
        "/api/v1/functions",
        "/api/v1/functions/probe.ping",
        "/api/v1/call/probe.ping",
        "/api/v1/i18n",
    ] {
        let (status, body) = probe(path).await;
        assert_eq!(status, 503, "{path}: {body}");
        assert_eq!(body["ok"], json!(false), "{path}: {body}");
        assert_eq!(
            body["error"]["code"],
            json!("unavailable"),
            "{path}: {body}"
        );
        assert_eq!(
            body["error"]["key"],
            json!("keel.error.unavailable"),
            "{path}: {body}"
        );
        assert!(body["error"]["requestId"].is_string(), "{path}: {body}");
    }
    let (status, body) = probe("/nowhere").await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));
    let kernel = Kernel::boot(Composition::new().module(&PROBE), &storage)
        .await
        .expect("the kernel boots while the listener serves");
    let dispatcher = Arc::new(
        Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles"),
    );
    http.attach(dispatcher.clone())
        .expect("the kernel attaches once");
    assert!(
        matches!(http.attach(dispatcher), Err(HttpError::Attached)),
        "a second kernel is refused"
    );
    let (status, body) = probe("/readyz").await;
    assert_eq!(status, 503);
    assert_eq!(body, json!({ "status": "migrating" }));
    readiness.migrated();
    let (status, body) = probe("/readyz").await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({ "status": "ready" }));
    let (status, body) = probe("/api/v1/call/probe.ping").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["level"], json!("guest"));
    readiness.draining();
    let (status, body) = probe("/readyz").await;
    assert_eq!(status, 503);
    assert_eq!(body, json!({ "status": "draining" }));
    let (status, _) = probe("/healthz").await;
    assert_eq!(status, 200);
    served.stop().await.expect("the server stops");
    kernel.stop().await.expect("the kernel stops");
    storage.close().await.expect("the pool closes");
    database.finish().await;
}

#[tokio::test]
async fn the_server_binds_a_free_port_and_stops_on_request() {
    let bench = bench().await;
    let http = Http::new(
        bench.storage.db().clone(),
        bench.readiness.clone(),
        HttpOptions::default(),
    );
    http.attach(bench.dispatcher.clone())
        .expect("the kernel attaches once");
    let served = http.bind_local().await.expect("port 0 binds");
    let addr = served.addr();
    assert_ne!(addr.port(), 0);
    assert_eq!(served.url("/healthz"), format!("http://{addr}/healthz"));
    let (status, headers, body) = raw(
        addr,
        "GET /healthz HTTP/1.1\r\nHost: keel.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status, 200, "{headers}");
    assert!(
        headers.to_ascii_lowercase().contains("x-keel-request-id:"),
        "{headers}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({ "status": "ok" })
    );
    let (status, _, body) = raw(
        addr,
        "POST /api/v1/call/probe.ping HTTP/1.1\r\nHost: keel.test\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    )
    .await;
    assert_eq!(status, 200);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["data"]["level"], json!("guest"));
    served.stop().await.expect("the server stops");
    assert!(TcpStream::connect(addr).await.is_err());
    bench.finish().await;
}

async fn raw(addr: SocketAddr, request: &str) -> (u16, String, String) {
    let Ok(mut stream) = TcpStream::connect(addr).await else {
        return (0, String::new(), String::new());
    };
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return (0, text, String::new());
    };
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0);
    (status, head.to_owned(), body.to_owned())
}

#[tokio::test]
async fn functions_as_a_guest_list_only_guest_level_functions() {
    let bench = bench().await;
    let answer = bench.get("/api/v1/functions", None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.header(CACHE_CONTROL), Some("private, no-store"));
    assert!(answer.header(ETAG).is_some());
    let descriptors = &answer.body;
    assert!(descriptors.is_array(), "{descriptors}");
    assert_eq!(
        names(descriptors),
        ["probe.client", "probe.cookie", "probe.fail", "probe.ping"]
    );
    for descriptor in descriptors.as_array().unwrap() {
        assert_eq!(descriptor["access"]["level"], json!("guest"));
        assert_eq!(descriptor["keel"], json!(1));
    }
    let as_user = bench.get("/api/v1/functions", Some(&user())).await;
    let listed = names(&as_user.body);
    assert!(listed.contains(&"keel.account.list"), "{listed:?}");
    assert!(listed.contains(&"keel.principal.create"), "{listed:?}");
    assert!(!listed.contains(&"keel.account.create"), "{listed:?}");
    assert!(!listed.contains(&"probe.admin"), "{listed:?}");
    let as_admin = bench.get("/api/v1/functions", Some(&admin())).await;
    let listed = names(&as_admin.body);
    assert!(listed.contains(&"keel.account.create"), "{listed:?}");
    assert!(listed.contains(&"probe.admin"), "{listed:?}");
    bench.finish().await;
}

#[tokio::test]
async fn an_unknown_function_yields_the_not_found_envelope() {
    let bench = bench().await;
    let call = bench
        .call("nope.thing.list", Some(&user()), json!({}))
        .await;
    assert_eq!(call.status, StatusCode::NOT_FOUND);
    assert_eq!(call.code(), ErrorCode::NotFound);
    assert_eq!(call.key(), "keel.error.function_not_found");
    assert_eq!(call.body["error"]["requestId"], json!(call.request_id()));
    let get = bench.get("/api/v1/call/nope.thing.list?page=1", None).await;
    assert_eq!(get.status, StatusCode::NOT_FOUND);
    assert_eq!(get.code(), ErrorCode::NotFound);
    let descriptor = bench.get("/api/v1/functions/nope.thing.list", None).await;
    assert_eq!(descriptor.status, StatusCode::NOT_FOUND);
    assert_eq!(descriptor.key(), "keel.error.function_not_found");
    let route = bench.get("/api/v2/anything", None).await;
    assert_eq!(route.status, StatusCode::NOT_FOUND);
    assert_eq!(route.key(), "keel.error.route_not_found");
    assert!(route.header(REQUEST_ID).is_some());
    bench.finish().await;
}

#[tokio::test]
async fn keel_account_list_is_unauthenticated_for_a_guest_and_a_page_for_a_user() {
    let bench = bench().await;
    let guest = bench.call("keel.account.list", None, json!({})).await;
    assert_eq!(guest.status, StatusCode::UNAUTHORIZED);
    assert_eq!(guest.body["ok"], json!(false));
    assert_eq!(guest.code(), ErrorCode::Unauthenticated);
    assert_eq!(guest.key(), "keel.error.unauthenticated");
    assert!(guest.header(META_VERSION).is_some());
    let user = bench
        .call("keel.account.list", Some(&user()), json!({}))
        .await;
    assert_eq!(user.status, StatusCode::OK, "{}", user.body);
    let page = user.data();
    assert_eq!(page["total"], json!(1));
    assert_eq!(page["data"][0]["id"], json!(CUSTOMER));
    assert!(page["data"][0].get("parentId").is_none());
    assert_eq!(page["totals"], Value::Null);
    assert_eq!(user.body["meta"]["principal"]["account"], json!(CUSTOMER));
    assert!(user.header(META_VERSION).unwrap().starts_with("m-"));
    assert!(user.header(IDEMPOTENT_REPLAY).is_none());
    bench.finish().await;
}

#[tokio::test]
async fn every_error_code_maps_to_its_documented_status() {
    let bench = bench().await;
    let expected = [
        (ErrorCode::Validation, 422),
        (ErrorCode::Forbidden, 403),
        (ErrorCode::ForbiddenField, 403),
        (ErrorCode::Unauthenticated, 401),
        (ErrorCode::NotFound, 404),
        (ErrorCode::Conflict, 409),
        (ErrorCode::IdempotencyMismatch, 409),
        (ErrorCode::LimitExceeded, 409),
        (ErrorCode::License, 402),
        (ErrorCode::RateLimited, 429),
        (ErrorCode::Unavailable, 503),
        (ErrorCode::Peer, 502),
        (ErrorCode::Internal, 500),
    ];
    assert_eq!(expected.len(), ErrorCode::ALL.len());
    for (code, status) in expected {
        let answer = bench
            .call("probe.fail", None, json!({ "code": code.as_str() }))
            .await;
        assert_eq!(answer.status.as_u16(), status, "{code:?}: {}", answer.body);
        assert_eq!(answer.code(), code);
        assert_eq!(answer.key(), format!("keel.error.{}", code.as_str()));
        assert_eq!(answer.body["error"]["params"]["raised"], json!(true));
        assert_eq!(
            answer.body["error"]["requestId"],
            json!(answer.request_id())
        );
        assert!(answer.header(META_VERSION).is_some(), "{code:?}");
        let over_get = bench
            .get(
                &format!("/api/v1/call/probe.fail?code={}", code.as_str()),
                None,
            )
            .await;
        assert_eq!(over_get.status.as_u16(), status, "{code:?} over GET");
    }
    bench.finish().await;
}

#[tokio::test]
async fn every_response_carries_a_request_id_and_a_plain_client_id_is_kept() {
    let bench = bench().await;
    for path in [
        "/healthz",
        "/readyz",
        "/api/v1/functions",
        "/api/v1/i18n",
        "/nowhere",
    ] {
        let answer = bench.get(path, None).await;
        let id = answer.request_id();
        assert!(id.starts_with("req-"), "{path}: {id}");
    }
    let wrong_method = bench
        .send(
            Request::delete("/api/v1/functions")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(wrong_method.status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(wrong_method.request_id().starts_with("req-"));
    let generated = bench.call("probe.ping", None, json!({})).await;
    assert!(generated.request_id().starts_with("req-"));
    assert_eq!(
        generated.body["meta"]["requestId"],
        json!(generated.request_id())
    );
    let kept = bench
        .post(
            "probe.ping",
            None,
            &[("x-keel-request-id", "trace-42.a")],
            json!({}),
        )
        .await;
    assert_eq!(kept.request_id(), "trace-42.a");
    assert_eq!(kept.body["meta"]["requestId"], json!("trace-42.a"));
    let refused = bench
        .post(
            "probe.fail",
            None,
            &[("x-keel-request-id", "has space")],
            json!({ "code": "conflict" }),
        )
        .await;
    assert!(refused.request_id().starts_with("req-"));
    assert_eq!(
        refused.body["error"]["requestId"],
        json!(refused.request_id())
    );
    bench.finish().await;
}

#[tokio::test]
async fn the_descriptor_etag_changes_after_the_principals_acl_gen_is_bumped() {
    let bench = bench().await;
    let created = bench
        .as_owner(
            "keel.principal.create",
            json!({ "login": "alice", "level": "user", "account": CUSTOMER }),
        )
        .await;
    let alice = "login:alice";
    let first = bench
        .get("/api/v1/functions/keel.account.list", Some(alice))
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert_eq!(first.header(CACHE_CONTROL), Some("private, no-store"));
    let etag = first.header(ETAG).expect("an ETag").to_owned();
    assert!(etag.starts_with("\"m-") && etag.ends_with('"'), "{etag}");
    assert_eq!(first.body["name"], json!("keel.account.list"));
    assert!(first.body.get("ok").is_none(), "{}", first.body);
    assert!(first.body.get("data").is_none(), "{}", first.body);
    let again = bench
        .get("/api/v1/functions/keel.account.list", Some(alice))
        .await;
    assert_eq!(again.header(ETAG), Some(etag.as_str()));
    let listing = bench.get("/api/v1/functions", Some(alice)).await;
    assert_eq!(listing.header(ETAG), Some(etag.as_str()));
    let call = bench.call("probe.ping", Some(alice), json!({})).await;
    assert_eq!(call.header(META_VERSION), Some(etag.trim_matches('"')));
    let cached = bench
        .send(
            Request::get("/api/v1/functions/keel.account.list")
                .header(AUTHORIZATION, format!("Bearer {alice}"))
                .header(IF_NONE_MATCH, etag.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    assert_eq!(cached.body, Value::Null);
    assert_eq!(cached.header(ETAG), Some(etag.as_str()));
    assert!(cached.header(REQUEST_ID).is_some());
    bench
        .as_owner(
            "keel.principal.update",
            json!({ "id": created["id"], "role": "viewer" }),
        )
        .await;
    let bumped = bench
        .send(
            Request::get("/api/v1/functions/keel.account.list")
                .header(AUTHORIZATION, format!("Bearer {alice}"))
                .header(IF_NONE_MATCH, etag.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(bumped.status, StatusCode::OK);
    let fresh = bumped.header(ETAG).expect("an ETag").to_owned();
    assert_ne!(fresh, etag);
    let other = bench.get("/api/v1/functions", Some(&user())).await;
    assert_ne!(other.header(ETAG), Some(fresh.as_str()));
    bench.finish().await;
}

#[tokio::test]
async fn descriptors_are_projected_by_level_and_permission() {
    let bench = bench().await;
    let columns = |descriptor: &Value| -> Vec<String> {
        descriptor["ui"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column["key"].as_str().unwrap().to_owned())
            .collect()
    };
    let actions = |descriptor: &Value| -> Vec<String> {
        descriptor["ui"]["actions"]
            .as_array()
            .map(|actions| {
                actions
                    .iter()
                    .map(|action| action["id"].as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    let as_user = bench
        .get("/api/v1/functions/keel.account.list", Some(&user()))
        .await;
    let list = &as_user.body;
    assert_eq!(columns(list), ["id", "kind", "state", "createdAt"]);
    assert!(actions(list).is_empty(), "{}", list["ui"]);
    assert!(list["ui"].get("row_actions").is_none(), "{}", list["ui"]);
    assert!(
        list["ui"]["empty"].get("action").is_none(),
        "{}",
        list["ui"]
    );
    let as_reseller = bench
        .get("/api/v1/functions/keel.account.list", Some(&reseller()))
        .await;
    let list = &as_reseller.body;
    assert_eq!(
        columns(list),
        ["id", "kind", "state", "parentId", "createdAt"]
    );
    assert_eq!(actions(list), ["create", "edit"]);
    assert_eq!(list["ui"]["row_actions"], json!(["edit"]));
    assert_eq!(list["ui"]["empty"]["action"], json!("create"));
    let as_admin = bench
        .get("/api/v1/functions/keel.account.list", Some(&admin()))
        .await;
    assert_eq!(
        columns(&as_admin.body),
        ["id", "kind", "state", "parentId", "createdAt", "origin"]
    );
    let form = bench
        .get("/api/v1/functions/keel.principal.create", Some(&user()))
        .await;
    let form = &form.body;
    assert!(
        form["params"]["properties"].get("account").is_none(),
        "{}",
        form["params"]
    );
    assert!(form["params"]["properties"].get("login").is_some());
    assert_eq!(
        form["ui"]["pages"][0]["fields"],
        json!(["login", "level", "role", "lang"])
    );
    let form = bench
        .get("/api/v1/functions/keel.principal.create", Some(&reseller()))
        .await;
    assert!(form.body["params"]["properties"].get("account").is_some());
    let guest = bench.get("/api/v1/functions/keel.account.list", None).await;
    assert_eq!(guest.status, StatusCode::UNAUTHORIZED);
    let below = bench
        .get("/api/v1/functions/probe.admin", Some(&user()))
        .await;
    assert_eq!(below.status, StatusCode::FORBIDDEN);
    assert_eq!(below.code(), ErrorCode::Forbidden);
    bench.finish().await;
}

#[tokio::test]
async fn the_idempotency_key_feeds_the_call_and_a_replay_is_marked() {
    let bench = bench().await;
    let header = IDEMPOTENCY_KEY;
    let key = [(header.as_str(), "open-1")];
    let first = bench
        .post(
            "keel.account.create",
            Some(&reseller()),
            &key,
            json!({ "kind": "user" }),
        )
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    assert!(first.header(IDEMPOTENT_REPLAY).is_none());
    let id = first.data()["id"].clone();
    let replay = bench
        .post(
            "keel.account.create",
            Some(&reseller()),
            &key,
            json!({ "kind": "user" }),
        )
        .await;
    assert_eq!(replay.status, StatusCode::OK);
    assert_eq!(replay.header(IDEMPOTENT_REPLAY), Some("true"));
    assert_eq!(replay.data()["id"], id);
    let mismatch = bench
        .post(
            "keel.account.create",
            Some(&reseller()),
            &key,
            json!({ "kind": "client" }),
        )
        .await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
    assert_eq!(mismatch.code(), ErrorCode::IdempotencyMismatch);
    assert!(mismatch.header(IDEMPOTENT_REPLAY).is_none());
    let count = bench
        .database
        .count(
            &bench.storage,
            "SELECT count(*) FROM keel_account WHERE kind = 'user'",
        )
        .await;
    assert_eq!(count, 2);
    bench.finish().await;
}

#[tokio::test]
async fn session_cookie_requests_are_checked_for_cross_site_and_other_requests_are_not() {
    let bench = bench().await;
    let cookie = format!("{SESSION_COOKIE}={}", user());
    let send = |headers: Vec<(&'static str, String)>| {
        let router = bench.router.clone();
        async move {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri("/api/v1/call/probe.ping")
                .header(HOST, "keel.test:1500");
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let response = router
                .oneshot(request.body(Body::from("{}")).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let body: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                    .unwrap();
            (status, body)
        }
    };
    let (status, body) = send(vec![
        (COOKIE.as_str(), cookie.clone()),
        ("sec-fetch-site", "cross-site".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], json!("forbidden"));
    assert_eq!(body["error"]["key"], json!("keel.error.cross_site"));
    let (status, body) = send(vec![
        (COOKIE.as_str(), cookie.clone()),
        (ORIGIN.as_str(), "https://evil.test".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = send(vec![
        ("cookie", "theme=dark".into()),
        (ORIGIN.as_str(), "https://evil.test".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.error.cross_site"));
    let (status, body) = send(vec![
        (COOKIE.as_str(), cookie.clone()),
        (ORIGIN.as_str(), "http://keel.test:1500".into()),
        ("sec-fetch-site", "same-origin".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["level"], json!("user"));
    let (status, body) = send(vec![(COOKIE.as_str(), cookie.clone())]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(vec![
        (AUTHORIZATION.as_str(), format!("Bearer {}", user())),
        ("sec-fetch-site", "cross-site".into()),
        (ORIGIN.as_str(), "https://evil.test".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(vec![
        (AUTHORIZATION.as_str(), format!("Bearer {}", user())),
        (COOKIE.as_str(), cookie),
        ("sec-fetch-site", "cross-site".into()),
    ])
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(vec![(ORIGIN.as_str(), "https://evil.test".into())]).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], json!("forbidden"));
    assert_eq!(body["error"]["key"], json!("keel.error.cross_site"));
    let (status, body) = send(vec![("sec-fetch-site", "cross-site".into())]).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.error.cross_site"));
    let (status, body) = send(vec![]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["level"], json!("guest"));
    bench.finish().await;
}

#[tokio::test]
async fn the_body_limit_is_enforced_and_a_non_json_body_is_refused() {
    let bench = bench_with(
        Composition::new().module(&PROBE),
        HttpOptions {
            body_limit: 64,
            ..HttpOptions::default()
        },
    )
    .await;
    let small = bench.call("probe.ping", None, json!({})).await;
    assert_eq!(small.status, StatusCode::OK);
    let large = bench
        .call("probe.ping", None, json!({ "padding": "x".repeat(100) }))
        .await;
    assert_eq!(
        large.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        large.body
    );
    assert_eq!(large.code(), ErrorCode::Validation);
    assert_eq!(large.key(), "keel.error.body_too_large");
    assert_eq!(large.body["error"]["params"]["limit"], json!(64));
    let broken = bench
        .send(
            Request::post("/api/v1/call/probe.ping")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await;
    assert_eq!(broken.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(broken.key(), "keel.error.body_not_json");
    let empty = bench
        .send(
            Request::post("/api/v1/call/probe.ping")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.body);
    bench.finish().await;
}

#[tokio::test]
async fn get_is_accepted_for_list_and_get_with_parameters_from_the_query() {
    let bench = bench().await;
    let list = bench
        .get(
            "/api/v1/call/keel.account.list?pageSize=1&sort=%7B%22key%22%3A%22id%22%2C%22dir%22%3A%22desc%22%7D",
            Some(&admin()),
        )
        .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    assert_eq!(list.data()["total"], json!(3));
    assert_eq!(list.data()["data"].as_array().unwrap().len(), 1);
    assert_eq!(list.data()["data"][0]["id"], json!(CUSTOMER));
    let get = bench
        .get(
            &format!("/api/v1/call/keel.account.get?id={ROOT}"),
            Some(&admin()),
        )
        .await;
    assert_eq!(get.status, StatusCode::OK, "{}", get.body);
    assert_eq!(get.data()["kind"], json!("system"));
    let json = bench
        .get(
            "/api/v1/call/keel.account.list?q=%7B%22search%22%3A%22reseller%22%7D&search=ignored",
            Some(&admin()),
        )
        .await;
    assert_eq!(json.status, StatusCode::OK, "{}", json.body);
    assert_eq!(json.data()["total"], json!(1));
    assert_eq!(json.data()["data"][0]["id"], json!(RESELLER));
    let bad = bench
        .get("/api/v1/call/keel.account.list?q=%5B1%5D", Some(&admin()))
        .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(bad.key(), "keel.error.query_not_object");
    let write = bench
        .get(
            "/api/v1/call/keel.account.create?kind=user",
            Some(&reseller()),
        )
        .await;
    assert_eq!(
        write.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        write.body
    );
    assert_eq!(write.code(), ErrorCode::Validation);
    assert_eq!(write.key(), "keel.error.method_not_allowed");
    assert_eq!(write.header(ALLOW), Some("POST"));
    let count = bench
        .database
        .count(&bench.storage, "SELECT count(*) FROM keel_account")
        .await;
    assert_eq!(count, 3);
    bench.finish().await;
}

#[tokio::test]
async fn i18n_serves_the_merged_catalogue_with_english_fallback() {
    let bench = bench().await;
    let en = bench.get("/api/v1/i18n", None).await;
    assert_eq!(en.status, StatusCode::OK, "{}", en.body);
    assert_eq!(en.data()["lang"], json!("en"));
    let messages = &en.data()["messages"];
    assert_eq!(messages["probe"]["greeting"], json!("Hello"));
    assert_eq!(
        messages["keel"]["error"]["validation"],
        json!("The request is not valid")
    );
    assert_eq!(messages["menu"]["system"], json!("System"));
    assert_eq!(en.header(CACHE_CONTROL), Some("private, no-store"));
    let etag = en.header(ETAG).expect("an ETag").to_owned();
    let ru = bench.get("/api/v1/i18n?lang=ru", None).await;
    assert_eq!(ru.data()["lang"], json!("ru"));
    let messages = &ru.data()["messages"];
    assert_eq!(messages["probe"]["greeting"], json!("ru:Hello"));
    assert_eq!(messages["probe"]["only_en"], json!("English only"));
    let kernel_ru: Value = serde_json::from_str(KERNEL_RU).expect("the kernel ru dictionary");
    assert_eq!(
        messages["keel"]["error"]["validation"],
        kernel_ru["keel"]["error"]["validation"]
    );
    assert_ne!(
        messages["keel"]["error"]["validation"],
        json!("The request is not valid")
    );
    assert_ne!(ru.header(ETAG), Some(etag.as_str()));
    let unknown = bench.get("/api/v1/i18n?lang=xx", None).await;
    assert_eq!(unknown.data()["lang"], json!("en"));
    assert_eq!(unknown.header(ETAG), Some(etag.as_str()));
    let cached = bench
        .send(
            Request::get("/api/v1/i18n")
                .header(IF_NONE_MATCH, etag.as_str())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    let spoken = bench
        .send(
            Request::post("/api/v1/call/probe.ping")
                .header("accept-language", "de-CH, ru-RU;q=0.8, en;q=0.5")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(spoken.data()["lang"], json!("ru"));
    let by_principal = bench
        .send(
            Request::get("/api/v1/i18n")
                .header("accept-language", "ru")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(by_principal.data()["lang"], json!("ru"));
    bench.finish().await;
}

#[tokio::test]
async fn an_invalid_credential_is_unauthenticated_and_an_authenticator_failure_is_reported() {
    let bench = bench().await;
    let stale = bench.call("probe.ping", Some("none"), json!({})).await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED);
    assert_eq!(stale.code(), ErrorCode::Unauthenticated);
    assert_eq!(stale.body["error"]["requestId"], json!(stale.request_id()));
    let stale_cookie = bench
        .send(
            Request::post("/api/v1/call/probe.ping")
                .header(COOKIE, format!("{SESSION_COOKIE}=none"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(stale_cookie.status, StatusCode::UNAUTHORIZED);
    let failed = bench.call("probe.ping", Some("boom"), json!({})).await;
    assert_eq!(failed.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(failed.code(), ErrorCode::Internal);
    let unknown_login = bench
        .call("probe.ping", Some("login:nobody"), json!({}))
        .await;
    assert_eq!(unknown_login.status, StatusCode::UNAUTHORIZED);
    bench.finish().await;
}

#[tokio::test]
async fn without_an_authenticator_every_credential_is_refused_and_a_bare_request_is_a_guest() {
    let bench = bench_with(Composition::new(), HttpOptions::default()).await;
    let with_token = bench
        .call("keel.account.list", Some(&admin()), json!({}))
        .await;
    assert_eq!(
        with_token.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        with_token.body
    );
    assert_eq!(with_token.code(), ErrorCode::Unauthenticated);
    let functions = bench.get("/api/v1/functions", Some(&admin())).await;
    assert_eq!(
        functions.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        functions.body
    );
    assert_eq!(functions.key(), "keel.error.unauthenticated");
    let with_cookie = bench
        .send(
            Request::get("/api/v1/functions")
                .header(COOKIE, format!("{SESSION_COOKIE}=anything"))
                .header(HOST, "keel.test")
                .header(ORIGIN, "http://keel.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(
        with_cookie.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        with_cookie.body
    );
    let bare = bench.get("/api/v1/functions", None).await;
    assert_eq!(bare.status, StatusCode::OK, "{}", bare.body);
    assert!(bare.body.as_array().unwrap().is_empty(), "{}", bare.body);
    bench.finish().await;
}

fn connected(mut request: Request<Body>) -> Request<Body> {
    let addr: SocketAddr = "127.0.0.1:40001".parse().expect("an address");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(addr));
    request
}

#[tokio::test]
async fn a_session_credential_carries_its_row_and_the_call_reads_the_client_of_the_request() {
    let bench = bench().await;
    let cookie = format!("{SESSION_COOKIE}={}", user());
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/call/probe.client")
        .header(COOKIE, cookie.clone())
        .header("user-agent", "probe/1")
        .body(Body::empty())
        .expect("a request");
    let answer = bench.send(connected(request)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(
        answer.data()["client"],
        json!({
            "ip": "127.0.0.1",
            "userAgent": "probe/1",
            "session": format!("row-of-{}", user())
        })
    );
    let bearer = bench.get("/api/v1/call/probe.client", Some(&user())).await;
    assert_eq!(bearer.status, StatusCode::OK, "{}", bearer.body);
    assert_eq!(
        bearer.data()["client"],
        json!({ "ip": null, "userAgent": null, "session": null })
    );
    let guest = bench.get("/api/v1/call/probe.client", None).await;
    assert_eq!(guest.data()["client"]["session"], json!(null));
    let direct = bench
        .dispatcher
        .call(
            principal(Level::Owner, Id::ROOT_ACCOUNT),
            "probe.client",
            json!({}),
            CallOptions::default(),
        )
        .await;
    assert_eq!(direct.data().expect("a reply")["client"], json!(null));
    bench.finish().await;
}

#[tokio::test]
async fn a_recorded_session_change_becomes_the_cookie_of_a_committed_call_only() {
    let bench = bench().await;
    let set = bench
        .call("probe.cookie", None, json!({ "change": "set" }))
        .await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    assert_eq!(
        set.header(axum::http::header::SET_COOKIE),
        Some("__Host-keel_session=ks_probe; Path=/; Secure; HttpOnly; SameSite=Lax")
    );
    let clear = bench
        .call("probe.cookie", None, json!({ "change": "clear" }))
        .await;
    assert_eq!(clear.status, StatusCode::OK, "{}", clear.body);
    assert_eq!(
        clear.header(axum::http::header::SET_COOKIE),
        Some("__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    let failed = bench
        .call(
            "probe.cookie",
            None,
            json!({ "change": "set", "fail": true }),
        )
        .await;
    assert_eq!(failed.status, StatusCode::CONFLICT, "{}", failed.body);
    assert_eq!(failed.key(), "probe.error.after_cookie");
    assert_eq!(failed.header(axum::http::header::SET_COOKIE), None);
    let none = bench
        .call("probe.cookie", None, json!({ "change": "none" }))
        .await;
    assert_eq!(none.status, StatusCode::OK, "{}", none.body);
    assert_eq!(none.header(axum::http::header::SET_COOKIE), None);
    bench.finish().await;
}

#[tokio::test]
async fn a_refused_session_cookie_is_cleared_and_a_refused_bearer_is_not() {
    let bench = bench().await;
    let stale = bench
        .send(
            Request::builder()
                .method(Method::GET)
                .uri("/api/v1/call/probe.ping")
                .header(COOKIE, format!("{SESSION_COOKIE}=nobody"))
                .body(Body::empty())
                .expect("a request"),
        )
        .await;
    assert_eq!(stale.status, StatusCode::UNAUTHORIZED, "{}", stale.body);
    assert_eq!(stale.key(), "keel.error.unauthenticated");
    assert_eq!(
        stale.header(axum::http::header::SET_COOKIE),
        Some("__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0")
    );
    let bearer = bench.get("/api/v1/call/probe.ping", Some("nobody")).await;
    assert_eq!(bearer.status, StatusCode::UNAUTHORIZED, "{}", bearer.body);
    assert_eq!(bearer.header(axum::http::header::SET_COOKIE), None);
    bench.finish().await;
}

struct Chain {
    name: &'static str,
    level: Level,
}

#[async_trait]
impl Authenticator for Chain {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        _: &Db,
    ) -> Result<Option<Authenticated>> {
        let token = match credential {
            Credential::Bearer(secret) | Credential::Session(secret) => secret.expose(),
        };
        let mut answered = principal(self.level, Id::ROOT_ACCOUNT);
        answered.login = self.name.to_owned();
        let mine = Some(Authenticated {
            principal: answered,
            session: None,
        });
        match (self.name, token) {
            ("first", "pass") => Ok(None),
            ("first", "take") => Ok(mine),
            ("first", "err") => Err(KeelError::unauthenticated("keel.error.unauthenticated")),
            ("first", _) => Ok(None),
            (_, "unknown") => Ok(None),
            _ => Ok(mine),
        }
    }
}

static FIRST: ModuleDef = ModuleDef {
    id: "first",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: first_init,
};

static SECOND: ModuleDef = ModuleDef {
    id: "second",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: second_init,
};

fn first_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Nothing, Pong>("first.ping")
            .kind(Kind::Get)
            .level(Level::Guest)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                Ok(Pong {
                    login: call.principal().login.clone(),
                    level: call.principal().level,
                    lang: call.principal().lang.clone(),
                })
            }),
    )
    .contribute::<Arc<dyn Authenticator>>(Arc::new(Chain {
        name: "first",
        level: Level::Admin,
    }));
    Ok(())
}

fn second_init(r: &mut Registrar) -> Result<()> {
    r.contribute::<Arc<dyn Authenticator>>(Arc::new(Chain {
        name: "second",
        level: Level::User,
    }));
    Ok(())
}

#[tokio::test]
async fn authenticators_are_asked_in_module_order_until_one_answers_or_fails() {
    let bench = bench_with(
        Composition::new().module(&SECOND).module(&FIRST),
        HttpOptions::default(),
    )
    .await;
    let passed = bench.get("/api/v1/call/first.ping", Some("pass")).await;
    assert_eq!(passed.status, StatusCode::OK, "{}", passed.body);
    assert_eq!(passed.data()["login"], json!("second"));
    let taken = bench.get("/api/v1/call/first.ping", Some("take")).await;
    assert_eq!(taken.status, StatusCode::OK, "{}", taken.body);
    assert_eq!(taken.data()["login"], json!("first"));
    assert_eq!(taken.data()["level"], json!("admin"));
    let failed = bench.get("/api/v1/call/first.ping", Some("err")).await;
    assert_eq!(failed.status, StatusCode::UNAUTHORIZED, "{}", failed.body);
    assert_eq!(failed.code(), ErrorCode::Unauthenticated);
    let unknown = bench.get("/api/v1/call/first.ping", Some("unknown")).await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED, "{}", unknown.body);
    assert_eq!(unknown.key(), "keel.error.unauthenticated");
    let guest = bench.get("/api/v1/call/first.ping", None).await;
    assert_eq!(guest.data()["level"], json!("guest"));
    bench.finish().await;
}

#[tokio::test]
async fn every_request_without_a_bearer_token_is_checked_for_cross_site() {
    let bench = bench().await;
    let origin = bench
        .post(
            "keel.account.list",
            None,
            &[("origin", "https://evil.example")],
            json!({}),
        )
        .await;
    assert_eq!(origin.status, StatusCode::FORBIDDEN, "{}", origin.body);
    assert_eq!(origin.code(), ErrorCode::Forbidden);
    assert_eq!(origin.key(), "keel.error.cross_site");
    let fetch = bench
        .post(
            "keel.account.list",
            None,
            &[("sec-fetch-site", "cross-site")],
            json!({}),
        )
        .await;
    assert_eq!(fetch.status, StatusCode::FORBIDDEN, "{}", fetch.body);
    assert_eq!(fetch.key(), "keel.error.cross_site");
    let guest = bench.get("/api/v1/call/keel.account.list", None).await;
    assert_eq!(guest.status, StatusCode::UNAUTHORIZED, "{}", guest.body);
    assert_eq!(guest.code(), ErrorCode::Unauthenticated);
    assert_eq!(guest.key(), "keel.error.unauthenticated");
    let bearer = bench
        .post(
            "keel.account.list",
            Some(&user()),
            &[("origin", "https://evil.example")],
            json!({}),
        )
        .await;
    assert_eq!(bearer.status, StatusCode::OK, "{}", bearer.body);
    bench.finish().await;
}

fn behind_proxies() -> HttpOptions {
    HttpOptions {
        trusted_proxies: ["127.0.0.0/8", "10.0.0.0/8"]
            .iter()
            .map(|net| net.parse().expect("a net"))
            .collect(),
        ..HttpOptions::default()
    }
}

async fn client_seen(bench: &Bench, headers: &[(&str, &str)], connected_peer: bool) -> Value {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/call/probe.client")
        .header(COOKIE, format!("{SESSION_COOKIE}={}", user()));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::empty()).expect("a request");
    let answer = bench
        .send(if connected_peer {
            connected(request)
        } else {
            request
        })
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    answer.data()["client"]["ip"].clone()
}

#[tokio::test]
async fn x_forwarded_for_is_honoured_from_a_trusted_proxy_alone_and_the_rightmost_untrusted_entry_wins()
 {
    let behind = bench_with(Composition::new().module(&PROBE), behind_proxies()).await;
    let chain = [(FORWARDED_FOR, "203.0.113.9, 198.51.100.2, 10.1.2.3")];
    assert_eq!(
        client_seen(&behind, &chain, true).await,
        json!("198.51.100.2")
    );
    assert_eq!(
        client_seen(&behind, &[(FORWARDED_FOR, "203.0.113.9")], true).await,
        json!("203.0.113.9")
    );
    assert_eq!(
        client_seen(&behind, &[(FORWARDED_FOR, "10.1.1.1, 127.0.0.2")], true).await,
        json!("127.0.0.1"),
        "a chain of trusted proxies alone leaves the peer address"
    );
    assert_eq!(
        client_seen(&behind, &chain, false).await,
        json!(null),
        "without a peer address nothing is trusted"
    );
    assert_eq!(client_seen(&behind, &[], true).await, json!("127.0.0.1"));
    behind.finish().await;

    let direct = bench().await;
    assert_eq!(
        client_seen(&direct, &chain, true).await,
        json!("127.0.0.1"),
        "an untrusted peer cannot forward an address"
    );
    direct.finish().await;
}

async fn same_site_post(bench: &Bench, headers: &[(&str, &str)]) -> Answer {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/call/probe.ping")
        .header(HOST, "upstream.internal:1500")
        .header(COOKIE, format!("{SESSION_COOKIE}={}", user()))
        .header(CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    bench
        .send(connected(
            request.body(Body::from("{}")).expect("a request"),
        ))
        .await
}

#[tokio::test]
async fn a_same_site_post_is_admitted_by_the_forwarded_host_behind_a_trusted_proxy_alone() {
    let behind = bench_with(Composition::new().module(&PROBE), behind_proxies()).await;
    let forwarded = [
        (FORWARDED_HOST, "panel.example"),
        (ORIGIN.as_str(), "https://panel.example"),
    ];
    let admitted = same_site_post(&behind, &forwarded).await;
    assert_eq!(admitted.status, StatusCode::OK, "{}", admitted.body);
    assert_eq!(admitted.data()["level"], json!("user"));
    let with_port = same_site_post(
        &behind,
        &[
            (FORWARDED_HOST, "panel.example:8443"),
            (ORIGIN.as_str(), "https://PANEL.example:8443"),
        ],
    )
    .await;
    assert_eq!(with_port.status, StatusCode::OK, "{}", with_port.body);
    let foreign = same_site_post(
        &behind,
        &[
            (FORWARDED_HOST, "panel.example"),
            (ORIGIN.as_str(), "https://upstream.internal:1500"),
        ],
    )
    .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN, "{}", foreign.body);
    assert_eq!(foreign.key(), "keel.error.cross_site");
    behind.finish().await;

    let direct = bench().await;
    let refused = same_site_post(&direct, &forwarded).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert_eq!(refused.key(), "keel.error.cross_site");
    let host_stands = same_site_post(
        &direct,
        &[
            (FORWARDED_HOST, "panel.example"),
            (ORIGIN.as_str(), "https://upstream.internal:1500"),
        ],
    )
    .await;
    assert_eq!(host_stands.status, StatusCode::OK, "{}", host_stands.body);
    direct.finish().await;
}

#[tokio::test]
async fn a_malformed_forwarded_for_or_host_falls_back_to_the_peer_and_the_host_header() {
    let behind = bench_with(Composition::new().module(&PROBE), behind_proxies()).await;
    for malformed in ["nowhere", "203.0.113.9:4321", "203.0.113.9, unknown", ""] {
        assert_eq!(
            client_seen(&behind, &[(FORWARDED_FOR, malformed)], true).await,
            json!("127.0.0.1"),
            "{malformed:?}"
        );
    }
    for malformed in [
        "https://panel.example",
        "panel.example/admin",
        "a.example, b.example",
        "",
    ] {
        let stands = same_site_post(
            &behind,
            &[
                (FORWARDED_HOST, malformed),
                (ORIGIN.as_str(), "https://upstream.internal:1500"),
            ],
        )
        .await;
        assert_eq!(
            stands.status,
            StatusCode::OK,
            "{malformed:?}: {}",
            stands.body
        );
        let refused = same_site_post(
            &behind,
            &[
                (FORWARDED_HOST, malformed),
                (ORIGIN.as_str(), "https://panel.example"),
            ],
        )
        .await;
        assert_eq!(
            refused.status,
            StatusCode::FORBIDDEN,
            "{malformed:?}: {}",
            refused.body
        );
    }
    let boot = bench_with(Composition::new().module(&PROBE), behind_proxies()).await;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/boot")
        .header(HOST, "upstream.internal:1500")
        .header(FORWARDED_HOST, "panel.example/admin")
        .body(Body::empty())
        .expect("a request");
    let answer = boot.send(connected(request)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    boot.finish().await;
    behind.finish().await;
}
