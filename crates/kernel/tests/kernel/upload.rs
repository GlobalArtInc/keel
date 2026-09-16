use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use keel_kernel::dispatcher::{
    FILE_ENCODING_KEY, FILE_TOO_LARGE_KEY, FILE_TYPE_KEY, RAW_PREFIX, UPLOAD_ROUTE_KEY,
};
use keel_kernel::http::{
    BODY_NOT_MULTIPART_KEY, IDEMPOTENCY_KEY, IDEMPOTENT_REPLAY, PART_REPEATED_KEY, PART_UNKNOWN_KEY,
};
use keel_kernel::{Composition, Kernel};
use keel_sdk::Registrar;
use keel_sdk::multipart;
use keel_sdk::prelude::*;
use keel_sdk::system::{AuditEvent, AuditSink};
use keel_testkit::{Answer, Harness, TestDatabase, UploadPart, Who};
use keel_types::{Envelope, ErrorCode};

const ROUTE: &str = "/api/v1/x/locker/note.put";
const MAX_TEXT: u64 = 32;
const MAX_BLOB: u64 = 8;
const EN: &str = r#"{ "locker": { "note": { "put": { "title": "Put a note" }, "label": "Label", "text": "Text", "blob": "Blob", "error": { "empty": "The note is empty" } } } }"#;
const LOOSE_EN: &str =
    r#"{ "loose": { "note": { "put": { "title": "Put a note" }, "text": "Text" } } }"#;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Put {
    #[serde(default)]
    label: String,
    #[schemars(extend("x-keel" = multipart::file(MAX_TEXT, ["text/plain", "text/markdown"]), "x-ui" = json!({ "control": "file" })))]
    text: String,
    #[schemars(extend("contentEncoding" = "base64", "x-keel" = multipart::file(MAX_BLOB, ["application/octet-stream"]), "x-ui" = json!({ "control": "file" })))]
    blob: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Stored {
    label: String,
    text: String,
    blob: Option<String>,
    chars: usize,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Loose {
    #[schemars(extend("x-ui" = json!({ "control": "file" })))]
    text: String,
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
            .filter(|event| event.request_id == request_id)
            .cloned()
            .collect()
    }
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);

static LOCKER: ModuleDef = ModuleDef {
    id: "locker",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

static LOOSE: ModuleDef = ModuleDef {
    id: "loose",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: loose_init,
};

static FORMLESS: ModuleDef = ModuleDef {
    id: "loose",
    version: "0.1.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init: formless_init,
};

fn put() -> FunctionDef {
    function::<Put, Stored>("locker.note.put")
        .level(Level::User)
        .unowned()
        .ui(Form::new("locker.note.put.title")
            .page("main", ["label", "text", "blob"])
            .field("label")
            .readonly_if("$screen.params.label"))
        .upload(|call: Call<Put>| async move {
            let params = call.params();
            call.tx()?;
            if params.text.is_empty() {
                return Err(KeelError::validation("locker.note.error.empty").field("text"));
            }
            Ok(Stored {
                label: params.label.clone(),
                text: params.text.clone(),
                blob: params.blob.clone(),
                chars: params.text.chars().count(),
            })
        })
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(put())
        .provide::<dyn AuditSink>(RECORDER.clone())
        .i18n("en", EN);
    Ok(())
}

fn loose_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Loose, Stored>("loose.note.put")
            .level(Level::User)
            .unowned()
            .ui(Form::new("loose.note.put.title").page("main", ["text"]))
            .upload(|_: Call<Loose>| async { unreachable!() }),
    )
    .i18n("en", LOOSE_EN);
    Ok(())
}

fn formless_init(r: &mut Registrar) -> Result<()> {
    r.function(
        function::<Put, Stored>("loose.note.put")
            .level(Level::User)
            .unowned()
            .upload(|_: Call<Put>| async { unreachable!() }),
    )
    .i18n("en", LOOSE_EN);
    Ok(())
}

async fn boot() -> Harness {
    Harness::new().module(&LOCKER).boot().await
}

fn text(bytes: &[u8]) -> UploadPart<'_> {
    UploadPart::new("text", "note.txt", "text/plain; charset=utf-8", bytes)
}

fn refused_field(answer: &Answer) -> (String, String, Value) {
    assert_eq!(answer.status, 422, "{}", answer.body);
    let envelope = answer.json();
    assert_eq!(envelope["error"]["code"], json!("validation"));
    let fields = envelope["error"]["fields"]
        .as_array()
        .expect("field errors")
        .clone();
    assert_eq!(fields.len(), 1, "{envelope}");
    (
        fields[0]["path"].as_str().unwrap_or_default().to_owned(),
        fields[0]["key"].as_str().unwrap_or_default().to_owned(),
        fields[0]["params"].clone(),
    )
}

#[keel_testkit::tokio::test]
async fn a_multipart_upload_on_its_raw_route_lands_the_file_in_the_parameters_and_the_audit_record()
{
    let harness = boot().await;
    let http = harness.http().await;
    let answer = http
        .upload_with(
            Who::A1,
            ROUTE,
            &[(IDEMPOTENCY_KEY.as_str(), "put-1")],
            &json!({ "label": "first" }),
            &[
                text("hello, file".as_bytes()),
                UploadPart::new(
                    "blob",
                    "b.bin",
                    "application/octet-stream",
                    &[0, 255, 10, 13],
                ),
            ],
        )
        .await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let envelope = answer.json();
    assert_eq!(envelope["ok"], json!(true));
    assert_eq!(
        envelope["data"],
        json!({ "label": "first", "text": "hello, file", "blob": "AP8KDQ==", "chars": 11 })
    );
    assert_eq!(answer.header(IDEMPOTENT_REPLAY.as_str()), None);
    let request_id = answer
        .header("x-keel-request-id")
        .expect("the request id")
        .to_owned();
    let audited = RECORDER.of(&request_id);
    assert_eq!(audited.len(), 1);
    assert_eq!(audited[0].function, "locker.note.put");
    assert_eq!(audited[0].mode, Mode::Write);
    assert_eq!(audited[0].principal.login, "a1");
    assert_eq!(
        audited[0].params,
        json!({ "label": "first", "text": "hello, file", "blob": "AP8KDQ==" })
    );
    assert!(audited[0].outcome.is_committed());

    let replayed = http
        .upload_with(
            Who::A1,
            ROUTE,
            &[(IDEMPOTENCY_KEY.as_str(), "put-1")],
            &json!({ "label": "first" }),
            &[
                text("hello, file".as_bytes()),
                UploadPart::new(
                    "blob",
                    "b.bin",
                    "application/octet-stream",
                    &[0, 255, 10, 13],
                ),
            ],
        )
        .await;
    assert_eq!(replayed.status, 200, "{}", replayed.body);
    assert_eq!(replayed.header(IDEMPOTENT_REPLAY.as_str()), Some("true"));
    assert_eq!(replayed.json()["data"], envelope["data"]);

    let without_params = http
        .upload(Who::A2, ROUTE, &json!({}), &[text(b"only text")])
        .await;
    assert_eq!(without_params.status, 200, "{}", without_params.body);
    assert_eq!(
        without_params.json()["data"],
        json!({ "label": "", "text": "only text", "blob": null, "chars": 9 })
    );

    let empty = http.upload(Who::A1, ROUTE, &json!({}), &[text(b"")]).await;
    let (path, key, _) = refused_field(&empty);
    assert_eq!(
        (path.as_str(), key.as_str()),
        ("text", "locker.note.error.empty")
    );
    let failed = RECORDER.of(empty.header("x-keel-request-id").expect("the request id"));
    assert_eq!(failed.len(), 1);
    assert!(!failed[0].outcome.is_committed());

    let guest = http
        .upload(Who::Guest, ROUTE, &json!({}), &[text(b"x")])
        .await;
    assert_eq!(guest.status, 401, "{}", guest.body);
    let unknown = http
        .upload(
            Who::A1,
            &format!("{RAW_PREFIX}/locker/nothing"),
            &json!({}),
            &[text(b"x")],
        )
        .await;
    assert_eq!(unknown.status, 404, "{}", unknown.body);
    let getting = http.get(Who::A1, ROUTE).await;
    assert_eq!(getting.status, 422, "{}", getting.body);
    assert_eq!(getting.header("allow"), Some("POST"));
    assert_eq!(
        getting.json()["error"]["key"],
        json!("keel.error.method_not_allowed")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_part_above_max_bytes_or_of_another_type_or_not_utf8_is_refused_on_the_field() {
    let harness = boot().await;
    let http = harness.http().await;

    let big = vec![b'a'; MAX_TEXT as usize + 1];
    let too_large = http.upload(Who::A1, ROUTE, &json!({}), &[text(&big)]).await;
    let (path, key, params) = refused_field(&too_large);
    assert_eq!((path.as_str(), key.as_str()), ("text", FILE_TOO_LARGE_KEY));
    assert_eq!(params["limit"], json!(MAX_TEXT));
    assert_eq!(params.get("size"), None);
    let at_limit = vec![b'a'; MAX_TEXT as usize];
    let accepted = http
        .upload(Who::A1, ROUTE, &json!({}), &[text(&at_limit)])
        .await;
    assert_eq!(accepted.status, 200, "{}", accepted.body);

    let other_type = http
        .upload(
            Who::A1,
            ROUTE,
            &json!({}),
            &[UploadPart::new("text", "note.png", "image/png", b"png")],
        )
        .await;
    let (path, key, params) = refused_field(&other_type);
    assert_eq!((path.as_str(), key.as_str()), ("text", FILE_TYPE_KEY));
    assert_eq!(params["type"], json!("image/png"));
    assert_eq!(params["types"], json!(["text/plain", "text/markdown"]));
    let markdown = http
        .upload(
            Who::A1,
            ROUTE,
            &json!({}),
            &[UploadPart::new("text", "note.md", "text/markdown", b"# md")],
        )
        .await;
    assert_eq!(markdown.status, 200, "{}", markdown.body);

    let not_utf8 = http
        .upload(Who::A1, ROUTE, &json!({}), &[text(&[0xff, 0xfe, b'a'])])
        .await;
    let (path, key, _) = refused_field(&not_utf8);
    assert_eq!((path.as_str(), key.as_str()), ("text", FILE_ENCODING_KEY));

    let big_blob = vec![7u8; MAX_BLOB as usize + 1];
    let blob_too_large = http
        .upload(
            Who::A1,
            ROUTE,
            &json!({}),
            &[
                text(b"ok"),
                UploadPart::new("blob", "b.bin", "application/octet-stream", &big_blob),
            ],
        )
        .await;
    let (path, key, _) = refused_field(&blob_too_large);
    assert_eq!((path.as_str(), key.as_str()), ("blob", FILE_TOO_LARGE_KEY));

    let stray = http
        .upload(
            Who::A1,
            ROUTE,
            &json!({}),
            &[
                text(b"ok"),
                UploadPart::new("label", "l.txt", "text/plain", b"as a file"),
            ],
        )
        .await;
    assert_eq!(stray.status, 422, "{}", stray.body);
    assert_eq!(stray.json()["error"]["key"], json!(PART_UNKNOWN_KEY));
    assert_eq!(stray.json()["error"]["params"]["part"], json!("label"));

    let twice = http
        .upload(Who::A1, ROUTE, &json!({}), &[text(b"once"), text(b"again")])
        .await;
    assert_eq!(twice.status, 422, "{}", twice.body);
    assert_eq!(twice.json()["error"]["key"], json!(PART_REPEATED_KEY));
    assert_eq!(twice.json()["error"]["params"]["part"], json!("text"));

    let json_body = http
        .post(Who::A1, ROUTE, &json!({ "text": "inline" }))
        .await;
    assert_eq!(json_body.status, 422, "{}", json_body.body);
    assert_eq!(
        json_body.json()["error"]["key"],
        json!(BODY_NOT_MULTIPART_KEY)
    );

    let whole_body = vec![b'a'; (MAX_TEXT + MAX_BLOB) as usize + multipart::PARAMS_LIMIT + 1];
    let above_body = http
        .upload(Who::A1, ROUTE, &json!({}), &[text(&whole_body)])
        .await;
    assert_eq!(above_body.status, 422, "{}", above_body.body);
    assert_eq!(
        above_body.json()["error"]["key"],
        json!("keel.error.body_too_large")
    );
    assert_eq!(
        above_body.json()["error"]["params"]["limit"],
        json!(MAX_TEXT + MAX_BLOB + multipart::PARAMS_LIMIT as u64)
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_control_file_field_without_x_keel_file_fails_freeze() {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let error = Kernel::boot(Composition::new().module(&LOOSE), &storage)
        .await
        .expect_err("a file control needs x-keel.file");
    let text = error.to_string();
    assert!(
        text.contains(
            "function loose.note.put: field text has the file control without x-keel.file naming maxBytes and types"
        ),
        "{text}"
    );
    let error = Kernel::boot(Composition::new().module(&FORMLESS), &storage)
        .await
        .expect_err("an upload needs a form block");
    let text = error.to_string();
    assert!(
        text.contains(
            "function loose.note.put: the upload carries no form block to render its fields"
        ),
        "{text}"
    );
    storage.close().await.expect("the pool closes");
    database.finish().await;
}

#[keel_testkit::tokio::test]
async fn the_call_route_refuses_an_upload_and_the_socket_takes_the_field_inline_under_max_bytes() {
    let harness = boot().await;
    let http = harness.http().await;
    for answer in [
        http.post(
            Who::A1,
            "/api/v1/call/locker.note.put",
            &json!({ "text": "inline" }),
        )
        .await,
        http.get(Who::A1, "/api/v1/call/locker.note.put?text=inline")
            .await,
    ] {
        assert_eq!(answer.status, 422, "{}", answer.body);
        let envelope = answer.json();
        assert_eq!(envelope["error"]["key"], json!(UPLOAD_ROUTE_KEY));
        assert_eq!(envelope["error"]["params"]["route"], json!(ROUTE));
        assert_eq!(
            envelope["error"]["params"]["function"],
            json!("locker.note.put")
        );
    }
    http.stop().await.expect("the server stops");

    let socket = harness.socket().await;
    match socket
        .call(
            "locker.note.put",
            json!({ "label": "ctl", "text": "from the socket" }),
        )
        .await
    {
        Envelope::Ok { data, .. } => {
            assert_eq!(
                data,
                json!({ "label": "ctl", "text": "from the socket", "blob": null, "chars": 15 })
            );
        }
        Envelope::Err { error } => panic!("the socket refused the inline upload: {error:?}"),
    }
    let big = "a".repeat(MAX_TEXT as usize + 1);
    match socket
        .call(
            "locker.note.put",
            json!({ "text": big, "blob": "AAAAAAAAAAAAAAA=" }),
        )
        .await
    {
        Envelope::Err { error } => {
            assert_eq!(error.code, ErrorCode::Validation);
            let summary: Vec<(&str, &str)> = error
                .fields
                .iter()
                .map(|field| {
                    (
                        field.path.as_str(),
                        field.key.as_deref().unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                summary,
                [("text", FILE_TOO_LARGE_KEY), ("blob", FILE_TOO_LARGE_KEY)]
            );
            assert_eq!(error.fields[0].params["size"], json!(MAX_TEXT + 1));
            assert_eq!(error.fields[1].params["size"], json!(11));
        }
        Envelope::Ok { data, .. } => panic!("the socket took a file above maxBytes: {data}"),
    }
    let output = socket
        .ctl_json("locker.note.put", &["text=ctl text"], "en")
        .await;
    assert!(output.ok, "{}", output.text);
    assert!(output.text.contains("ctl text"), "{}", output.text);
    socket.stop().await.expect("the socket stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn a_cross_site_multipart_post_is_refused_403() {
    let harness = boot().await;
    let http = harness.http().await;
    for headers in [
        [("origin", "https://evil.example")],
        [("sec-fetch-site", "cross-site")],
    ] {
        let answer = http
            .upload_with(Who::Guest, ROUTE, &headers, &json!({}), &[text(b"x")])
            .await;
        assert_eq!(answer.status, 403, "{}", answer.body);
        assert_eq!(
            answer.json()["error"]["key"],
            json!("keel.error.cross_site")
        );
        assert!(
            RECORDER
                .of(answer.header("x-keel-request-id").expect("the request id"))
                .is_empty(),
            "a refused cross-site post never reaches the dispatcher"
        );
    }
    let same_site = http
        .upload_with(
            Who::Guest,
            ROUTE,
            &[("origin", "http://keel.test")],
            &json!({}),
            &[text(b"x")],
        )
        .await;
    assert_eq!(same_site.status, 401, "{}", same_site.body);
    assert_eq!(
        same_site.json()["error"]["key"],
        json!("keel.error.unauthenticated")
    );
    let bearer = http
        .upload_with(
            Who::A1,
            ROUTE,
            &[("origin", "https://evil.example")],
            &json!({}),
            &[text(b"x")],
        )
        .await;
    assert_eq!(bearer.status, 200, "{}", bearer.body);
    http.stop().await.expect("the server stops");
    harness.finish().await;
}

#[keel_testkit::tokio::test]
async fn an_upload_descriptor_carries_its_form_block() {
    let harness = boot().await;
    let http = harness.http().await;
    let answer = http.get(Who::A1, "/api/v1/functions/locker.note.put").await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let descriptor = answer.json();
    assert_eq!(descriptor["kind"], json!("upload"));
    assert_eq!(descriptor["mode"], json!("write"));
    assert_eq!(descriptor["ui"]["title"], json!("locker.note.put.title"));
    assert_eq!(
        descriptor["ui"]["pages"],
        json!([{ "id": "main", "fields": ["label", "text", "blob"] }])
    );
    assert_eq!(
        descriptor["ui"]["fields"]["label"]["readonly_if"],
        json!({ "truthy": "$screen.params.label" })
    );
    assert_eq!(
        descriptor["params"]["properties"]["text"]["x-keel"]["file"],
        json!({ "maxBytes": MAX_TEXT, "types": ["text/plain", "text/markdown"] })
    );
    assert_eq!(
        descriptor["params"]["properties"]["text"]["x-ui"]["control"],
        json!("file")
    );
    assert_eq!(
        descriptor["params"]["properties"]["blob"]["contentEncoding"],
        json!("base64")
    );
    http.stop().await.expect("the server stops");
    harness.finish().await;
}
