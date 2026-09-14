use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{
    ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    REFERRER_POLICY, VARY, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, Method, Request, StatusCode};
use keel_kernel::http::{Frontend, REQUEST_ID};
use keel_kernel::{Http, HttpOptions, Readiness, Storage};
use keel_testkit::TestDatabase;
use keel_testkit::tokio;
use serde_json::{Value, json};
use tower::ServiceExt;

const INDEX: &str = "<!doctype html><html><head><title>Keel</title></head><body class=\"phx-app\"><keel-shell></keel-shell><script src=\"main-abc123.js\" type=\"module\"></script></body></html>";
const MAIN: &[u8] = b"console.log('keel');";
const MAIN_BR: &[u8] = b"\x0b\x09\x80brotli";
const POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

fn frontend() -> Frontend {
    Frontend::from_files([
        ("index.html", INDEX.as_bytes()),
        ("main-abc123.js", MAIN),
        ("main-abc123.js.br", MAIN_BR),
    ])
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    router: Router,
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.bytes)
            .unwrap_or_else(|error| panic!("not JSON: {error}: {}", self.text()))
    }
}

async fn bench(frontend: Option<Frontend>) -> Bench {
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let readiness = Readiness::new();
    readiness.migrated();
    let http = Http::new(
        storage.db().clone(),
        readiness,
        HttpOptions {
            frontend,
            ..HttpOptions::default()
        },
    );
    Bench {
        database,
        storage,
        router: http.router(),
    }
}

impl Bench {
    async fn send(&self, method: Method, path: &str, headers: &[(&str, &str)]) -> Answer {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(request.body(Body::empty()).expect("a request"))
            .await
            .expect("the router answers");
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is read")
            .to_vec();
        Answer {
            status,
            headers,
            bytes,
        }
    }

    async fn get(&self, path: &str) -> Answer {
        self.send(Method::GET, path, &[]).await
    }

    async fn finish(self) {
        drop(self.router);
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn assert_index(answer: &Answer, path: &str) {
    assert_eq!(answer.status, StatusCode::OK, "{path}: {}", answer.text());
    assert_eq!(answer.text(), INDEX, "{path}");
    assert_eq!(
        answer.header(CONTENT_TYPE.as_str()),
        Some("text/html; charset=utf-8"),
        "{path}"
    );
    assert_eq!(
        answer.header(CACHE_CONTROL.as_str()),
        Some("no-store"),
        "{path}"
    );
    assert_eq!(
        answer.header(CONTENT_SECURITY_POLICY.as_str()),
        Some(POLICY),
        "{path}"
    );
    assert_eq!(
        answer.header(X_CONTENT_TYPE_OPTIONS.as_str()),
        Some("nosniff"),
        "{path}"
    );
    assert_eq!(
        answer.header(REFERRER_POLICY.as_str()),
        Some("same-origin"),
        "{path}"
    );
    assert!(answer.header(REQUEST_ID.as_str()).is_some(), "{path}");
}

#[tokio::test]
async fn the_root_and_a_screen_path_answer_the_index_no_store_with_the_policy_headers() {
    let bench = bench(Some(frontend())).await;
    for path in [
        "/",
        "/f/note.list",
        "/login",
        "/f/note.edit/01J8A0000000000000000000A2",
    ] {
        let answer = bench.get(path).await;
        assert_index(&answer, path);
    }
    let head = bench.send(Method::HEAD, "/f/note.list", &[]).await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.header(CACHE_CONTROL.as_str()), Some("no-store"));
    let index = bench.get("/index.html").await;
    assert_index(&index, "/index.html");
    bench.finish().await;
}

#[tokio::test]
async fn a_hashed_asset_is_immutable_for_a_year_and_has_a_brotli_twin_when_accepted() {
    let bench = bench(Some(frontend())).await;
    let plain = bench.get("/main-abc123.js").await;
    assert_eq!(plain.status, StatusCode::OK);
    assert_eq!(plain.bytes, MAIN);
    assert_eq!(
        plain.header(CONTENT_TYPE.as_str()),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        plain.header(CACHE_CONTROL.as_str()),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        plain.header(X_CONTENT_TYPE_OPTIONS.as_str()),
        Some("nosniff")
    );
    assert_eq!(plain.header(VARY.as_str()), Some("Accept-Encoding"));
    assert_eq!(plain.header(CONTENT_ENCODING.as_str()), None);
    assert_eq!(plain.header(CONTENT_SECURITY_POLICY.as_str()), None);
    let compressed = bench
        .send(
            Method::GET,
            "/main-abc123.js",
            &[(ACCEPT_ENCODING.as_str(), "gzip, deflate, br")],
        )
        .await;
    assert_eq!(compressed.status, StatusCode::OK);
    assert_eq!(compressed.bytes, MAIN_BR);
    assert_eq!(compressed.header(CONTENT_ENCODING.as_str()), Some("br"));
    assert_eq!(compressed.header(VARY.as_str()), Some("Accept-Encoding"));
    assert_eq!(
        compressed.header(CONTENT_TYPE.as_str()),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        compressed.header(CACHE_CONTROL.as_str()),
        Some("public, max-age=31536000, immutable")
    );
    let refused = bench
        .send(
            Method::GET,
            "/main-abc123.js",
            &[(ACCEPT_ENCODING.as_str(), "gzip, br;q=0")],
        )
        .await;
    assert_eq!(refused.bytes, MAIN);
    assert_eq!(refused.header(CONTENT_ENCODING.as_str()), None);
    let twin = bench.get("/main-abc123.js.br").await;
    assert_eq!(twin.status, StatusCode::NOT_FOUND);
    bench.finish().await;
}

#[tokio::test]
async fn a_missing_asset_is_404_while_the_api_keeps_its_envelope_and_the_probes_are_untouched() {
    let bench = bench(Some(frontend())).await;
    let missing = bench.get("/missing.png").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let body = missing.json();
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));
    assert_eq!(body["error"]["params"]["path"], json!("/missing.png"));
    for path in ["/media/x.woff2", "/chunk-ZPMDAVXX.js", "/f/note.list.js"] {
        let answer = bench.get(path).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            answer.json()["error"]["key"],
            json!("keel.error.route_not_found"),
            "{path}"
        );
    }
    let api = bench.get("/api/v1/nothing").await;
    assert_eq!(api.status, StatusCode::NOT_FOUND);
    let body = api.json();
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["error"]["code"], json!("not_found"));
    assert_eq!(body["error"]["key"], json!("keel.error.route_not_found"));
    assert!(body["error"]["requestId"].is_string());
    let brand = bench.get("/brand/logo.svg").await;
    assert_eq!(brand.status, StatusCode::NOT_FOUND);
    assert_eq!(
        brand.json()["error"]["key"],
        json!("keel.error.route_not_found")
    );
    let posted = bench.send(Method::POST, "/f/note.list", &[]).await;
    assert_eq!(posted.status, StatusCode::NOT_FOUND);
    assert_eq!(
        posted.json()["error"]["key"],
        json!("keel.error.route_not_found")
    );
    let health = bench.get("/healthz").await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(health.json(), json!({ "status": "ok" }));
    assert_eq!(health.header(CONTENT_SECURITY_POLICY.as_str()), None);
    bench.finish().await;
}

#[tokio::test]
async fn without_a_frontend_every_unknown_path_keeps_the_route_not_found_envelope() {
    let bench = bench(None).await;
    for path in ["/", "/f/note.list", "/main-abc123.js", "/index.html"] {
        let answer = bench.get(path).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            answer.json()["error"]["key"],
            json!("keel.error.route_not_found"),
            "{path}"
        );
    }
    bench.finish().await;
}
