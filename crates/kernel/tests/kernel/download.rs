use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_kernel::dispatcher::{DOWNLOAD_ROUTE_KEY, RAW_PREFIX};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_sdk::system::{AuditEvent, AuditSink};
use keel_testkit::{Answer, Harness, Who};
use keel_types::{Envelope, ErrorCode};

const ROUTE: &str = "/api/v1/x/vault/blob.fetch";
const EN: &str = r#"{ "vault": { "blob": { "name": "Name", "fail": "Fail", "error": { "locked": "The blob is locked" } } } }"#;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Fetch {
    #[serde(default = "default_name")]
    name: String,
    #[serde(default)]
    fail: bool,
}

fn default_name() -> String {
    "hello.txt".to_owned()
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Relayed {
    key: String,
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
    fn of(&self, function: &str) -> Vec<AuditEvent> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.function == function)
            .cloned()
            .collect()
    }
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);

static VAULT: ModuleDef = ModuleDef {
    id: "vault",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Fetch, DownloadFile>("vault.blob.fetch")
            .level(Level::User)
            .unowned()
            .download(|call: Call<Fetch>| async move {
                if call.params().fail {
                    return Err(KeelError::conflict("vault.blob.error.locked"));
                }
                let body = format!("hello {}\n", call.principal().login);
                Ok(Download::bytes(
                    call.params().name.clone(),
                    "text/plain; charset=utf-8",
                    body,
                )
                .into())
            }),
    )
    .function(
        function::<Nothing, Relayed>("vault.blob.relay")
            .kind(Kind::Action)
            .mode(Mode::Read)
            .level(Level::User)
            .unowned()
            .handler(|call: Call<Nothing>| async move {
                let error = call
                    .invoke("vault.blob.fetch", json!({}))
                    .await
                    .expect_err("a download cannot be invoked from another call");
                Ok(Relayed {
                    key: error.key().to_owned(),
                })
            }),
    )
    .provide::<dyn AuditSink>(RECORDER.clone())
    .i18n("en", EN);
    Ok(())
}

async fn boot() -> Harness {
    Harness::new().module(&VAULT).boot().await
}

fn refused_route(answer: &Answer) {
    assert_eq!(answer.status, 422, "{}", answer.body);
    let request_id = answer.header("x-keel-request-id").expect("the request id");
    assert!(
        RECORDER
            .of("vault.blob.fetch")
            .iter()
            .all(|event| event.request_id != request_id),
        "a refused call route is not audited"
    );
    let envelope = answer.json();
    assert_eq!(envelope["ok"], json!(false));
    assert_eq!(envelope["error"]["key"], json!(DOWNLOAD_ROUTE_KEY));
    assert_eq!(envelope["error"]["code"], json!("validation"));
    assert_eq!(envelope["error"]["params"]["route"], json!(ROUTE));
    assert_eq!(
        envelope["error"]["params"]["function"],
        json!("vault.blob.fetch")
    );
}

#[keel_testkit::tokio::test]
async fn a_download_is_served_and_audited_on_its_raw_route_and_refused_on_the_call_route_and_the_socket()
 {
    let harness = boot().await;
    let http = harness.http().await;

    let got = http.get(Who::A1, &format!("{ROUTE}?name=hello.txt")).await;
    assert_eq!(got.status, 200, "{}", got.body);
    assert_eq!(got.body, "hello a1\n");
    assert_eq!(
        got.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        got.header("content-disposition"),
        Some("attachment; filename=\"hello.txt\"")
    );
    assert_eq!(got.header("cache-control"), Some("no-store"));
    let request_id = got
        .header("x-keel-request-id")
        .expect("the request id header")
        .to_owned();

    let posted = http
        .post(Who::A2, ROUTE, &json!({ "name": "posted.txt" }))
        .await;
    assert_eq!(posted.status, 200, "{}", posted.body);
    assert_eq!(posted.body, "hello a2\n");
    assert_eq!(
        posted.header("content-disposition"),
        Some("attachment; filename=\"posted.txt\"")
    );

    let audited = RECORDER.of("vault.blob.fetch");
    let first = audited
        .iter()
        .find(|event| event.request_id == request_id)
        .expect("the read download is audited");
    assert_eq!(first.mode, Mode::Read);
    assert_eq!(first.principal.login, "a1");
    assert_eq!(first.params, json!({ "name": "hello.txt" }));
    assert!(first.outcome.is_committed());
    assert!(
        audited
            .iter()
            .any(|event| event.principal.login == "a2" && event.params["name"] == "posted.txt")
    );

    refused_route(
        &http
            .post(Who::A1, "/api/v1/call/vault.blob.fetch", &json!({}))
            .await,
    );
    refused_route(&http.get(Who::A1, "/api/v1/call/vault.blob.fetch").await);

    let socket = harness.socket().await;
    match socket.call("vault.blob.fetch", json!({})).await {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(error.key, DOWNLOAD_ROUTE_KEY);
            assert_eq!(error.params["route"], json!(ROUTE));
        }
        Envelope::Ok { data, .. } => panic!("the socket served a download: {data}"),
    }
    let output = socket.ctl_json("vault.blob.fetch", &[], "en").await;
    assert!(!output.ok, "{}", output.text);
    assert!(output.text.contains(DOWNLOAD_ROUTE_KEY), "{}", output.text);
    socket.stop().await.expect("the socket stops");

    let reply = harness
        .call(Who::A1, "vault.blob.fetch", json!({ "name": "direct.txt" }))
        .await;
    assert_eq!(
        reply.data(),
        Some(&json!({ "filename": "direct.txt", "contentType": "text/plain; charset=utf-8" }))
    );
    let download = reply.download.expect("the dispatcher hands the body over");
    assert_eq!(download.filename, "direct.txt");
    assert_eq!(download.collect().await.expect("the body"), b"hello a1\n");

    let relayed = harness.call(Who::A1, "vault.blob.relay", json!({})).await;
    assert_eq!(
        relayed.data(),
        Some(&json!({ "key": DOWNLOAD_ROUTE_KEY })),
        "{:?}",
        relayed.error()
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn an_error_before_the_first_byte_answers_the_envelope_and_a_non_ascii_name_gets_filename_star()
 {
    let harness = boot().await;
    let http = harness.http().await;
    let failed = http
        .post(Who::A1, ROUTE, &json!({ "name": "x.txt", "fail": true }))
        .await;
    assert_eq!(failed.status, 409, "{}", failed.body);
    assert!(
        failed
            .header("content-type")
            .is_some_and(|value| value.starts_with("application/json")),
        "{:?}",
        failed.headers
    );
    assert_eq!(failed.header("content-disposition"), None);
    let envelope = failed.json();
    assert_eq!(envelope["ok"], json!(false));
    assert_eq!(envelope["error"]["key"], json!("vault.blob.error.locked"));
    assert_eq!(envelope["error"]["code"], json!("conflict"));
    let request_id = failed.header("x-keel-request-id").expect("the request id");
    assert_eq!(envelope["error"]["requestId"], json!(request_id));
    let audited = RECORDER.of("vault.blob.fetch");
    let event = audited
        .iter()
        .find(|event| event.request_id == request_id)
        .expect("the failed download is audited");
    assert!(!event.outcome.is_committed());

    let named = http
        .get(
            Who::A1,
            &format!(
                "{ROUTE}?name={}",
                "%D0%B7%D0%B0%D0%BC%D0%B5%D1%82%D0%BA%D0%B8.txt"
            ),
        )
        .await;
    assert_eq!(named.status, 200, "{}", named.body);
    assert_eq!(
        named.header("content-disposition"),
        Some(
            "attachment; filename=\"_______.txt\"; filename*=UTF-8''%D0%B7%D0%B0%D0%BC%D0%B5%D1%82%D0%BA%D0%B8.txt"
        )
    );
    assert_eq!(named.body, "hello a1\n");
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_guest_is_refused_on_the_raw_route() {
    let harness = boot().await;
    let http = harness.http().await;
    let guest = http.get(Who::Guest, ROUTE).await;
    assert_eq!(guest.status, 401, "{}", guest.body);
    let envelope = guest.json();
    assert_eq!(
        envelope["error"]["key"],
        json!("keel.error.unauthenticated")
    );
    assert_eq!(envelope["error"]["code"], json!("unauthenticated"));

    let unknown = http
        .get(Who::A1, &format!("{RAW_PREFIX}/vault/nothing"))
        .await;
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    assert_eq!(
        unknown.json()["error"]["key"],
        json!("keel.error.route_not_found")
    );
    let not_a_download = http
        .get(Who::A1, &format!("{RAW_PREFIX}/keel/account.list"))
        .await;
    assert_eq!(not_a_download.status, 404, "{}", not_a_download.body);
    assert_eq!(
        not_a_download.json()["error"]["key"],
        json!("keel.error.route_not_found")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
