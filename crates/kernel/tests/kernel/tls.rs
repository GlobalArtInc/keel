use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use keel_kernel::dispatcher::Dispatcher;
use keel_kernel::http::{Scheme, TLS_SHUTDOWN_GRACE};
use keel_kernel::tls::{Names, TlsMaterial, install_provider};
use keel_kernel::{Composition, Http, HttpOptions, Kernel, Readiness, Storage};
use keel_sdk::Registrar;
use keel_sdk::prelude::*;
use keel_testkit::TestDatabase;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SLOW: Duration = Duration::from_secs(60);

#[derive(Deserialize, Serialize, JsonSchema)]
struct Nothing {}

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
        function::<Nothing, Nothing>("probe.slow")
            .kind(Kind::Get)
            .level(Level::Guest)
            .unowned()
            .handler(|_: Call<Nothing>| async {
                tokio::time::sleep(SLOW).await;
                Ok(Nothing {})
            }),
    );
    Ok(())
}

struct Bench {
    database: TestDatabase,
    storage: Storage,
    kernel: Kernel,
    http: Http,
}

async fn bench() -> Bench {
    install_provider();
    let database = TestDatabase::create().await;
    let storage = database.storage().await;
    let kernel = Kernel::boot(Composition::new().module(&PROBE), &storage)
        .await
        .expect("the kernel boots");
    let dispatcher = Arc::new(
        Dispatcher::new(kernel.registry().clone(), storage.db().clone())
            .expect("every schema compiles"),
    );
    let readiness = Readiness::new();
    readiness.migrated();
    let http = Http::new(storage.db().clone(), readiness, HttpOptions::default());
    http.attach(dispatcher).expect("the kernel attaches once");
    Bench {
        database,
        storage,
        kernel,
        http,
    }
}

impl Bench {
    async fn finish(self) {
        self.kernel.stop().await.expect("the kernel stops");
        self.storage.close().await.expect("the pool closes");
        self.database.finish().await;
    }
}

fn fresh_material() -> TlsMaterial {
    TlsMaterial::generate(&Names::for_host(
        Some("keel.test"),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    ))
    .expect("a pair")
}

fn client(trusting: &TlsMaterial) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([
            reqwest::Certificate::from_pem(trusting.certificate_pem().as_bytes())
                .expect("the certificate parses"),
        ])
        .timeout(Duration::from_secs(10))
        .build()
        .expect("a client")
}

#[tokio::test]
async fn bind_tls_serves_healthz_to_a_client_trusting_the_certificate_and_refuses_one_that_does_not()
 {
    let bench = bench().await;
    let material = fresh_material();
    let served = bench
        .http
        .clone()
        .bind_tls(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            &material,
        )
        .await
        .expect("port 0 binds over TLS");
    let addr = served.addr();
    assert_ne!(addr.port(), 0);
    assert_eq!(served.scheme(), Scheme::Https);
    assert_eq!(served.url("/healthz"), format!("https://{addr}/healthz"));

    let trusting = client(&material);
    let response = trusting
        .get(served.url("/healthz"))
        .send()
        .await
        .expect("the trusting client is answered");
    assert_eq!(response.status(), 200);
    assert!(response.headers().contains_key("x-keel-request-id"));
    let body: Value = response.json().await.expect("a JSON body");
    assert_eq!(body, json!({ "status": "ok" }));
    let call = trusting
        .post(served.url("/api/v1/call/probe.slow"))
        .header("content-type", "application/json")
        .body("{}")
        .timeout(Duration::from_millis(500))
        .send()
        .await;
    assert!(
        call.is_err(),
        "probe.slow does not answer within its timeout"
    );

    let doubting = client(&fresh_material());
    let refused = doubting.get(served.url("/healthz")).send().await;
    let error = refused.expect_err("an untrusted certificate is refused");
    assert!(error.is_connect() || error.is_request(), "{error}");
    assert!(
        !format!("{error:?}").contains("status"),
        "the refusal happens before any HTTP answer: {error:?}"
    );

    let mut plain = TcpStream::connect(addr).await.expect("the port accepts");
    plain
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: keel.test\r\nConnection: close\r\n\r\n")
        .await
        .expect("the plain request is written");
    let mut answer = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(15), plain.read_to_end(&mut answer)).await;
    let text = String::from_utf8_lossy(&answer);
    assert!(
        !text.starts_with("HTTP/"),
        "a plain request gets no HTTP answer, got {text:?} ({read:?})"
    );

    served.stop().await.expect("the server stops");
    assert!(TcpStream::connect(addr).await.is_err());
    bench.finish().await;
}

#[tokio::test]
async fn bind_tls_stops_within_its_grace_with_a_stream_open() {
    let bench = bench().await;
    let material = fresh_material();
    let served = bench
        .http
        .clone()
        .bind_tls(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            &material,
        )
        .await
        .expect("port 0 binds over TLS");
    let trusting = client(&material);
    let ready = trusting
        .get(served.url("/healthz"))
        .send()
        .await
        .expect("the listener answers");
    assert_eq!(ready.status(), 200);
    let url = served.url("/api/v1/call/probe.slow");
    let slow = tokio::spawn(async move {
        trusting
            .post(url)
            .header("content-type", "application/json")
            .body("{}")
            .timeout(SLOW)
            .send()
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!slow.is_finished(), "the slow call is still in flight");

    let started = Instant::now();
    served.stop().await.expect("the server stops");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= TLS_SHUTDOWN_GRACE,
        "the grace is waited out with a request in flight: {elapsed:?}"
    );
    assert!(
        elapsed < TLS_SHUTDOWN_GRACE + Duration::from_secs(10),
        "the stop does not wait for the slow handler: {elapsed:?}"
    );
    let outcome = tokio::time::timeout(Duration::from_secs(10), slow)
        .await
        .expect("the slow call is released by the stop")
        .expect("the task joins");
    assert!(
        outcome.is_err(),
        "the open request is cut, not answered: {outcome:?}"
    );
    bench.finish().await;
}
