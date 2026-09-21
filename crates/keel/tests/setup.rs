use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use keel::sdk::prelude::*;
use keel::{Config, Running, Runtime, TlsMaterial};
use keel_kernel::keel::{SETUP_KEY_PREFIX, SETUP_PATH};
use keel_kernel::{Log, LogFormat, logging, logs};
use keel_testkit::TestDatabase;
use tempfile::TempDir;
use tokio::net::UnixDatagram;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);
const SETUP_ADMIN: &str = "keel.setup.admin";
const LOGIN: &str = "first";
const PASSWORD: &str = "correct-horse-battery";
const SHORT_PASSWORD: &str = "short";
const NOTIFY_SOCKET: &str = "NOTIFY_SOCKET";
const WATCHDOG_USEC: &str = "WATCHDOG_USEC";
const WATCHDOG_INTERVAL_USEC: &str = "200000";

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Site {
    database: TestDatabase,
    dir: TempDir,
    config: Config,
}

async fn site() -> Site {
    let database = TestDatabase::create().await;
    let dir = tempfile::tempdir().expect("a temporary directory");
    let mut config = database.config();
    config.listen = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    config.socket = dir.path().join("run").join("core.sock");
    config.data_dir = dir.path().join("lib");
    Site {
        database,
        dir,
        config,
    }
}

impl Site {
    fn runtime(&self) -> Runtime {
        Runtime::new().module(&keel_system::auth::MODULE)
    }

    async fn finish(self) {
        drop(self.dir);
        self.database.finish().await;
    }
}

fn client(trusting: &TlsMaterial) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([
            reqwest::Certificate::from_pem(trusting.certificate_pem().as_bytes())
                .expect("the certificate parses"),
        ])
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("a client")
}

async fn post(
    running: &Running,
    func: &str,
    body: Value,
    cookie: Option<&str>,
) -> (u16, Option<String>, Value) {
    let mut request = client(running.tls())
        .post(running.url(&format!("/api/v1/call/{func}")))
        .header("host", "keel.test")
        .header("content-type", "application/json")
        .body(body.to_string());
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let response = request.send().await.expect("the listener answers");
    let status = response.status().as_u16();
    let cookie = response
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .filter(|value| value.starts_with(keel_kernel::http::SESSION_COOKIE))
        .map(str::to_owned);
    let text = response.text().await.expect("the body is read");
    (
        status,
        cookie,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn get(running: &Running, path: &str, cookie: &str) -> (u16, Value) {
    let response = client(running.tls())
        .get(running.url(path))
        .header("host", "keel.test")
        .header("cookie", cookie)
        .send()
        .await
        .expect("the listener answers");
    let status = response.status().as_u16();
    let text = response.text().await.expect("the body is read");
    (status, serde_json::from_str(&text).expect("a JSON body"))
}

fn capture_logs() {
    let _ = logging::init(&Log {
        level: "keel::runtime=info".to_owned(),
        format: LogFormat::Text,
    });
}

fn logged_link(running: &Running) -> Option<String> {
    let prefix = format!("https://{}{SETUP_PATH}?key=", running.http_addr());
    logs::ring()
        .tail(logs::CAPACITY, None)
        .into_iter()
        .filter_map(|entry| entry.fields.get("url")?.as_str().map(str::to_owned))
        .find(|url| url.starts_with(&prefix))
}

fn setup_body(key: &str, login: &str, password: &str) -> Value {
    json!({ "key": key, "login": login, "password": password })
}

#[tokio::test]
async fn the_setup_link_creates_the_first_administrator_once_and_the_function_is_gone_after() {
    let _serial = SERIAL.lock().await;
    capture_logs();
    let site = site().await;
    let running = site.runtime().start(&site.config).await.unwrap();
    let link = logged_link(&running).expect("the start logs the setup link");
    let prefix = format!("https://{}{SETUP_PATH}?key=", running.http_addr());
    let key = link.strip_prefix(&prefix).unwrap().to_owned();
    assert!(key.starts_with(SETUP_KEY_PREFIX), "{key}");

    let (status, _, body) = post(
        &running,
        SETUP_ADMIN,
        setup_body(&key[..key.len() - 1], LOGIN, PASSWORD),
        None,
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.setup.error.key"));

    let (status, _, body) = post(
        &running,
        SETUP_ADMIN,
        setup_body(&key, LOGIN, SHORT_PASSWORD),
        None,
    )
    .await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(
        body["error"]["fields"][0]["key"],
        json!("auth.error.password_short")
    );

    let (status, cookie, body) = post(
        &running,
        SETUP_ADMIN,
        setup_body(&key, LOGIN, PASSWORD),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["login"], json!(LOGIN));
    assert!(cookie.is_none(), "the setup opens no session of its own");
    let id = body["data"]["id"].as_str().expect("an id").to_owned();

    let (status, cookie, body) = post(
        &running,
        "auth.login",
        json!({ "login": LOGIN, "password": PASSWORD }),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["state"], json!("authenticated"));
    let cookie = cookie.expect("the login sets the session cookie");
    let (status, boot) = get(&running, "/api/v1/boot", &cookie).await;
    assert_eq!(status, 200, "{boot}");
    assert_eq!(boot["principal"]["login"], json!(LOGIN));
    assert_eq!(boot["principal"]["level"], json!("admin"));
    assert_eq!(boot["principal"]["id"], json!(id));

    let (status, _, body) = post(
        &running,
        SETUP_ADMIN,
        setup_body(&key, "second", PASSWORD),
        None,
    )
    .await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["key"], json!("keel.error.function_not_found"));
    assert_eq!(body["error"]["params"]["function"], json!(SETUP_ADMIN));
    running.stop().await.unwrap();

    let again = site.runtime().start(&site.config).await.unwrap();
    assert_eq!(
        logged_link(&again),
        None,
        "a start with an administrator prints no link"
    );
    let (status, _, body) = post(
        &again,
        SETUP_ADMIN,
        setup_body(&key, "third", PASSWORD),
        None,
    )
    .await;
    assert_eq!(status, 404, "{body}");
    again.stop().await.unwrap();
    site.finish().await;
}

async fn receive(socket: &UnixDatagram) -> String {
    let mut buffer = [0u8; 256];
    let received = tokio::time::timeout(NOTIFY_TIMEOUT, socket.recv(&mut buffer))
        .await
        .expect("a notification arrives in time")
        .expect("the notify socket reads");
    String::from_utf8_lossy(&buffer[..received]).into_owned()
}

#[tokio::test]
async fn the_runtime_sends_ready_and_watchdog_over_the_notify_socket() {
    let _serial = SERIAL.lock().await;
    capture_logs();
    let site = site().await;
    let path = site.dir.path().join("notify.sock");
    let socket = UnixDatagram::bind(&path).expect("the test binds the notify socket");
    unsafe {
        std::env::set_var(NOTIFY_SOCKET, &path);
        std::env::set_var(WATCHDOG_USEC, WATCHDOG_INTERVAL_USEC);
    }
    let (stop, shutdown) = tokio::sync::oneshot::channel::<()>();
    let runtime = site.runtime();
    let config = site.config.clone();
    let served = tokio::spawn(async move {
        runtime
            .serve(&config, async move {
                let _ = shutdown.await;
            })
            .await
    });

    assert_eq!(receive(&socket).await, "READY=1\n");
    assert_eq!(receive(&socket).await, "WATCHDOG=1\n");
    assert_eq!(receive(&socket).await, "WATCHDOG=1\n");
    stop.send(()).expect("serve is waiting");
    let mut last = receive(&socket).await;
    while last == "WATCHDOG=1\n" {
        last = receive(&socket).await;
    }
    assert_eq!(last, "STOPPING=1\n");
    served
        .await
        .expect("serve finishes")
        .expect("serve stops cleanly");
    unsafe {
        std::env::remove_var(NOTIFY_SOCKET);
        std::env::remove_var(WATCHDOG_USEC);
    }
    site.finish().await;
}
