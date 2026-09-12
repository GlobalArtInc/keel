mod common;

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{Method, Request};
use keel_kernel::ctl::{self, Out};
use keel_kernel::dispatcher::{CallOptions, Dispatcher};
use keel_kernel::socket::{Client, ClientError, FRAME_LIMIT, OWNER_LOGIN, Socket, owner};
use keel_kernel::{Composition, Http, HttpOptions, Kernel, Readiness, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Authenticator, Credential};
use keel_types::{Envelope, ErrorCode, Via};
use tempfile::TempDir;
use tower::ServiceExt;

const EN: &str = r#"{ "probe": { "greeting": "Hello" } }"#;
const RU: &str = r#"{ "probe": { "greeting": "Привет" } }"#;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pong {
    login: String,
    level: Level,
    via: Via,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Fail {
    code: ErrorCode,
}

struct OwnerOverHttp;

#[async_trait]
impl Authenticator for OwnerOverHttp {
    async fn authenticate(&self, credential: &Credential, _: &Db) -> Result<Option<Principal>> {
        let token = match credential {
            Credential::Bearer(secret) | Credential::Session(secret) => secret.expose(),
        };
        if token != OWNER_LOGIN {
            return Ok(None);
        }
        let mut principal = owner();
        principal.via = Via::Http;
        Ok(Some(principal))
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
                    via: call.principal().via.clone(),
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
    .i18n("en", EN)
    .i18n("ru", RU)
    .provide::<dyn Authenticator>(Arc::new(OwnerOverHttp));
    Ok(())
}

struct Bench {
    database: common::TestDatabase,
    storage: Storage,
    kernel: Kernel,
    dispatcher: Arc<Dispatcher>,
    dir: TempDir,
}

async fn bench(composition: Composition) -> Bench {
    let database = common::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(composition, &storage)
        .await
        .expect("the kernel boots");
    let dispatcher = Arc::new(
        Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles"),
    );
    Bench {
        database,
        storage,
        kernel,
        dispatcher,
        dir: tempfile::tempdir().expect("a temporary directory"),
    }
}

impl Bench {
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    async fn socket(&self) -> keel_kernel::socket::Serving {
        Socket::new(self.dispatcher.clone())
            .start(&self.path("core.sock"))
            .await
            .expect("the socket starts")
    }

    async fn over_http(&self, func: &str, params: Value) -> Value {
        let readiness = Readiness::new();
        readiness.migrated();
        let http = Http::new(self.storage.db().clone(), readiness, HttpOptions::default());
        http.attach(self.dispatcher.clone())
            .expect("the kernel attaches once");
        let request = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/v1/call/{func}"))
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {OWNER_LOGIN}"))
            .body(Body::from(params.to_string()))
            .expect("a request");
        let response = http
            .router()
            .oneshot(request)
            .await
            .expect("the router answers");
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read");
        serde_json::from_slice(&bytes).expect("the body is JSON")
    }

    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.drop().await;
    }
}

fn without_meta(mut envelope: Value) -> Value {
    if let Some(object) = envelope.as_object_mut() {
        object.remove("meta");
        if let Some(error) = object.get_mut("error").and_then(Value::as_object_mut) {
            error.remove("requestId");
        }
    }
    envelope
}

fn to_value<T: Serialize>(envelope: &Envelope<T>) -> Value {
    serde_json::to_value(envelope).expect("an envelope serialises")
}

#[tokio::test]
async fn a_bare_kernel_answers_keel_account_list_on_the_socket_as_the_owner() {
    let bench = bench(Composition::new()).await;
    let serving = bench.socket().await;
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    let envelope = client
        .call("keel.account.list", json!({}), CallOptions::default())
        .await
        .expect("the socket answers");
    let Envelope::Ok { data, meta } = envelope else {
        panic!("expected an ok envelope: {envelope:?}");
    };
    assert_eq!(data["total"], json!(1));
    assert_eq!(data["data"][0]["id"], json!(Id::ROOT_ACCOUNT.to_string()));
    assert_eq!(data["data"][0]["kind"], json!("system"));
    let principal = meta.principal.expect("the owner is named in meta");
    assert_eq!(principal.level, Level::Owner);
    assert_eq!(principal.login, OWNER_LOGIN);
    assert_eq!(principal.account, Id::ROOT_ACCOUNT);
    assert!(principal.actor.is_none());
    assert!(meta.request_id.starts_with("req-"), "{}", meta.request_id);
    let ping = client
        .call(
            "probe.ping",
            json!({}),
            CallOptions {
                request_id: Some("cli-7".into()),
                idempotency_key: None,
            },
        )
        .await
        .expect("the socket answers");
    let Envelope::Err { error } = ping else {
        panic!("probe is not composed: {ping:?}");
    };
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.request_id.as_deref(), Some("cli-7"));
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}

#[tokio::test]
async fn the_socket_and_http_envelopes_for_the_same_call_differ_only_in_meta() {
    let bench = bench(Composition::new().module(&PROBE)).await;
    let serving = bench.socket().await;
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    for (func, params) in [
        ("keel.account.list", json!({})),
        ("probe.ping", json!({})),
        ("probe.fail", json!({ "code": "conflict" })),
        ("nowhere.at.all", json!({})),
    ] {
        let over_socket = to_value(
            &client
                .call(func, params.clone(), CallOptions::default())
                .await
                .expect("the socket answers"),
        );
        let over_http = bench.over_http(func, params).await;
        assert_eq!(over_socket["ok"], over_http["ok"], "{func}");
        if func == "probe.ping" {
            assert_eq!(over_socket["data"]["via"], json!("socket"));
            assert_eq!(over_http["data"]["via"], json!("http"));
            continue;
        }
        assert_eq!(
            without_meta(over_socket.clone()),
            without_meta(over_http.clone()),
            "{func}: {over_socket} vs {over_http}"
        );
        if over_socket["ok"] == json!(true) {
            assert_ne!(
                over_socket["meta"]["requestId"], over_http["meta"]["requestId"],
                "{func}"
            );
            assert_eq!(
                over_socket["meta"]["principal"]["login"],
                over_http["meta"]["principal"]["login"]
            );
        } else {
            assert_ne!(
                over_socket["error"]["requestId"], over_http["error"]["requestId"],
                "{func}"
            );
        }
    }
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}

#[tokio::test]
async fn describe_and_i18n_frames_answer_the_projected_descriptor_and_the_catalogue() {
    let bench = bench(Composition::new().module(&PROBE)).await;
    let serving = bench.socket().await;
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    let described = client
        .describe("keel.account.list")
        .await
        .expect("the socket answers");
    let Envelope::Ok { data, meta } = described else {
        panic!("expected a descriptor: {described:?}");
    };
    assert_eq!(data.name, "keel.account.list");
    let columns: Vec<&str> = data
        .ui
        .as_ref()
        .expect("a list has a screen")
        .columns
        .iter()
        .map(|column| column.key.as_str())
        .collect();
    assert_eq!(
        columns,
        ["id", "kind", "state", "parent", "created_at", "origin"]
    );
    assert_eq!(meta.principal.expect("the owner").level, Level::Owner);
    let missing = client
        .describe("nowhere.at.all")
        .await
        .expect("the socket answers");
    let Envelope::Err { error } = missing else {
        panic!("expected not_found: {missing:?}");
    };
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(error.key, "keel.error.function_not_found");
    let catalogue = client.i18n("ru").await.expect("the socket answers");
    let Envelope::Ok { data, .. } = catalogue else {
        panic!("expected a catalogue: {catalogue:?}");
    };
    assert_eq!(data.lang, "ru");
    assert_eq!(data.messages["probe"]["greeting"], json!("Привет"));
    assert_eq!(
        data.messages["keel"]["account"]["column"]["kind"],
        json!("Тип")
    );
    let fallback = client.i18n("xx").await.expect("the socket answers");
    let Envelope::Ok { data, .. } = fallback else {
        panic!("expected a catalogue: {fallback:?}");
    };
    assert_eq!(data.lang, "en");
    assert_eq!(data.messages["probe"]["greeting"], json!("Hello"));
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}

fn frame_of(len: usize) -> String {
    let head = r#"{"func":"keel.account.list","requestId":""#;
    let tail = r#""}"#;
    let frame = format!("{head}{}{tail}", "x".repeat(len - head.len() - tail.len()));
    assert_eq!(frame.len(), len);
    frame
}

#[tokio::test]
async fn malformed_frames_are_answered_with_an_error_envelope_and_the_connection_stays_open() {
    let bench = bench(Composition::new()).await;
    let serving = bench.socket().await;
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    for (line, key) in [
        ("{not json", "keel.error.body_not_json"),
        ("{}", "keel.error.frame_unknown"),
        (
            r#"{"describe":"a.b.c","i18n":"en"}"#,
            "keel.error.frame_unknown",
        ),
        (
            r#"{"func":"keel.account.list","extra":1}"#,
            "keel.error.frame_unknown",
        ),
    ] {
        let answer: Envelope<Value> = client.send_raw(line).await.expect("the socket answers");
        let Envelope::Err { error } = answer else {
            panic!("{line}: expected an error envelope: {answer:?}");
        };
        assert_eq!(error.code, ErrorCode::Validation, "{line}");
        assert_eq!(error.key, key, "{line}");
        assert!(error.request_id.is_some(), "{line}");
    }
    let answer: Envelope<Value> = client
        .send_raw("\n   \n{\"func\":\"keel.account.list\"}")
        .await
        .expect("blank lines are skipped");
    assert!(matches!(answer, Envelope::Ok { .. }), "{answer:?}");
    let answer: Envelope<Value> = client
        .send_raw(&frame_of(FRAME_LIMIT))
        .await
        .expect("a frame of exactly the limit is answered");
    assert!(matches!(answer, Envelope::Ok { .. }), "{answer:?}");
    let answer: Envelope<Value> = client
        .send_raw(&frame_of(FRAME_LIMIT + 1))
        .await
        .expect("the socket answers before closing");
    let Envelope::Err { error } = answer else {
        panic!("expected body_too_large: {answer:?}");
    };
    assert_eq!(error.key, "keel.error.body_too_large");
    assert_eq!(error.params["limit"], json!(FRAME_LIMIT));
    let closed = client
        .send_raw::<Value>(r#"{"func":"keel.account.list"}"#)
        .await;
    assert!(
        matches!(closed, Err(ClientError::Closed) | Err(ClientError::Io(_))),
        "{closed:?}"
    );
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}

#[tokio::test]
async fn a_stale_socket_file_is_replaced_and_a_live_one_is_refused() {
    let bench = bench(Composition::new()).await;
    let path = bench.path("nested/core.sock");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"stale").unwrap();
    let serving = Socket::new(bench.dispatcher.clone())
        .start(&path)
        .await
        .expect("a stale file is replaced");
    let mode =
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions());
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    let busy = Socket::new(bench.dispatcher.clone())
        .start(&path)
        .await
        .err()
        .expect("a live socket is refused");
    assert_eq!(
        busy.to_string(),
        format!("another kernel is listening on {}", path.display())
    );
    assert!(Client::connect(&path).await.is_ok());
    serving.stop().await.expect("the socket stops");
    assert!(!path.exists(), "the socket file is removed on stop");
    assert!(matches!(
        Client::connect(&path).await,
        Err(ClientError::Connect { .. })
    ));
    bench.finish().await;
}

#[tokio::test]
async fn ctl_renders_keel_account_list_as_a_table_and_creates_an_account_from_pairs() {
    let bench = bench(Composition::new()).await;
    let serving = bench.socket().await;
    let created = ctl::run(
        serving.path(),
        &ctl::Request {
            func: "keel.account.create".into(),
            params: ctl::params(["kind=reseller"]).unwrap(),
            out: Out::Json,
            lang: "en".into(),
        },
    )
    .await
    .expect("ctl runs");
    assert!(created.ok, "{}", created.text);
    let envelope: Value = serde_json::from_str(&created.text).expect("ctl prints the envelope");
    assert_eq!(envelope["ok"], json!(true));
    let id = envelope["data"]["id"].as_str().expect("the new id");
    assert!(id.parse::<Id>().is_ok(), "{id}");
    assert_eq!(envelope["meta"]["principal"]["level"], json!("owner"));

    let table = ctl::run(
        serving.path(),
        &ctl::Request {
            func: "keel.account.list".into(),
            params: json!({ "sort": { "key": "id", "dir": "asc" } }),
            out: Out::Table,
            lang: "en".into(),
        },
    )
    .await
    .expect("ctl runs");
    assert!(table.ok, "{}", table.text);
    let lines: Vec<&str> = table.text.lines().collect();
    assert_eq!(lines.len(), 3, "{}", table.text);
    let headers: Vec<&str> = lines[0].split_whitespace().collect();
    assert_eq!(
        headers,
        ["Account", "Kind", "State", "Parent", "Created", "Origin"]
    );
    assert!(
        lines[1].starts_with(&Id::ROOT_ACCOUNT.to_string()),
        "{}",
        lines[1]
    );
    assert!(lines[1].contains("system"), "{}", lines[1]);
    assert!(lines[2].starts_with(id), "{}", lines[2]);
    assert!(lines[2].contains("reseller"), "{}", lines[2]);

    let russian = ctl::run(
        serving.path(),
        &ctl::Request {
            func: "keel.account.list".into(),
            params: json!({}),
            out: Out::Table,
            lang: "ru".into(),
        },
    )
    .await
    .expect("ctl runs");
    assert!(russian.text.starts_with("Аккаунт"), "{}", russian.text);

    let failed = ctl::run(
        serving.path(),
        &ctl::Request {
            func: "keel.account.create".into(),
            params: json!({ "kind": "not a name" }),
            out: Out::Table,
            lang: "en".into(),
        },
    )
    .await
    .expect("ctl runs");
    assert!(!failed.ok);
    assert!(
        failed
            .text
            .starts_with("error: validation keel.error.validation"),
        "{}",
        failed.text
    );
    assert!(
        failed.text.contains("kind: keel.check.name"),
        "{}",
        failed.text
    );

    let object = ctl::run(
        serving.path(),
        &ctl::Request {
            func: "keel.account.get".into(),
            params: json!({ "id": id }),
            out: Out::Table,
            lang: "en".into(),
        },
    )
    .await
    .expect("ctl runs");
    assert!(object.ok, "{}", object.text);
    let first = object.text.lines().next().unwrap_or_default();
    assert!(first.starts_with("id "), "{}", object.text);
    assert!(first.ends_with(id), "{}", object.text);
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}

#[tokio::test]
async fn an_idempotency_key_in_the_frame_replays_the_stored_response() {
    let bench = bench(Composition::new()).await;
    let serving = bench.socket().await;
    let mut client = Client::connect(serving.path())
        .await
        .expect("the client connects");
    let options = || CallOptions {
        request_id: None,
        idempotency_key: Some("create-once".into()),
    };
    let first = client
        .call("keel.account.create", json!({ "kind": "user" }), options())
        .await
        .expect("the socket answers");
    let second = client
        .call("keel.account.create", json!({ "kind": "user" }), options())
        .await
        .expect("the socket answers");
    let (Envelope::Ok { data: one, .. }, Envelope::Ok { data: two, .. }) = (&first, &second) else {
        panic!("{first:?} {second:?}");
    };
    assert_eq!(one["id"], two["id"]);
    assert_eq!(
        bench
            .database
            .count(&bench.storage, "SELECT count(*) FROM keel_account")
            .await,
        2
    );
    let mismatch = client
        .call(
            "keel.account.create",
            json!({ "kind": "reseller" }),
            options(),
        )
        .await
        .expect("the socket answers");
    let Envelope::Err { error } = mismatch else {
        panic!("expected idempotency_mismatch: {mismatch:?}");
    };
    assert_eq!(error.code, ErrorCode::IdempotencyMismatch);
    serving.stop().await.expect("the socket stops");
    bench.finish().await;
}
