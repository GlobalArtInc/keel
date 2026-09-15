use std::future::Future;
use std::time::{Duration, Instant};

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::Caller;
use keel_system::notify::api::{KEY_PROBLEM_RAISED, KEY_TEST, Problems, Raise, Severity};
use keel_system::notify::{
    CHANNEL_GET, CHANNEL_SET, CHANNEL_TEST, DELIVERY_LIST, MODULE, STATE_SENT,
};
use keel_testkit::{Harness, Reply, Who, tokio};
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt, ReuseDirective};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const SMTP_VARIABLE: &str = "KEEL_TEST_SMTP_URL";
pub const API_VARIABLE: &str = "KEEL_TEST_SMTP_API";
pub const CATCHER_IMAGE: &str = "axllent/mailpit";
pub const CATCHER_TAG: &str = "v1.31.1";
pub const CATCHER_NAME: &str = "keel-testkit-mailpit";
const SMTP_PORT: u16 = 1025;
const API_PORT: u16 = 8025;
const PATIENCE: Duration = Duration::from_secs(30);
const READY_TIMEOUT: Duration = Duration::from_secs(60);

struct Catcher {
    smtp_host: String,
    smtp_port: u16,
    api: String,
    _container: Option<ContainerAsync<GenericImage>>,
}

fn host_and_port(url: &str, scheme: &str, variable: &str) -> (String, u16) {
    let rest = url
        .strip_prefix(scheme)
        .unwrap_or_else(|| panic!("{variable} must start with {scheme}: {url}"));
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = authority
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("{variable} must carry a port: {url}"));
    (
        host.to_owned(),
        port.parse()
            .unwrap_or_else(|_| panic!("{variable} carries the port {port}")),
    )
}

async fn catcher() -> Catcher {
    if let (Ok(smtp), Ok(api)) = (std::env::var(SMTP_VARIABLE), std::env::var(API_VARIABLE)) {
        let (smtp_host, smtp_port) = host_and_port(&smtp, "smtp://", SMTP_VARIABLE);
        let catcher = Catcher {
            smtp_host,
            smtp_port,
            api: api.trim_end_matches('/').to_owned(),
            _container: None,
        };
        wait_ready(&catcher.api).await;
        return catcher;
    }
    let container = GenericImage::new(CATCHER_IMAGE, CATCHER_TAG)
        .with_exposed_port(ContainerPort::Tcp(SMTP_PORT))
        .with_exposed_port(ContainerPort::Tcp(API_PORT))
        .with_wait_for(WaitFor::healthcheck())
        .with_container_name(CATCHER_NAME)
        .with_reuse(ReuseDirective::Always)
        .start()
        .await
        .unwrap_or_else(|error| {
            panic!("{SMTP_VARIABLE} is not set and the {CATCHER_IMAGE}:{CATCHER_TAG} container could not be started: {error}")
        });
    let host = container.get_host().await.expect("the container host");
    let smtp_port = container
        .get_host_port_ipv4(SMTP_PORT)
        .await
        .expect("the smtp port");
    let api_port = container
        .get_host_port_ipv4(API_PORT)
        .await
        .expect("the api port");
    let catcher = Catcher {
        smtp_host: host.to_string(),
        smtp_port,
        api: format!("http://{host}:{api_port}"),
        _container: Some(container),
    };
    wait_ready(&catcher.api).await;
    eprintln!(
        "keel-system: {SMTP_VARIABLE} is not set; using the {CATCHER_IMAGE}:{CATCHER_TAG} container {CATCHER_NAME} at {}",
        catcher.api
    );
    catcher
}

async fn request(api: &str, method: &str, path: &str) -> Option<(u16, String)> {
    let (host, port) = host_and_port(api, "http://", API_VARIABLE);
    let mut stream = TcpStream::connect((host.as_str(), port)).await.ok()?;
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())?;
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        unchunk(body)
    } else {
        body.to_owned()
    };
    Some((status, body))
}

fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        out.push_str(&after[..size.min(after.len())]);
        rest = after.get(size + 2..).unwrap_or("");
    }
    out
}

async fn wait_ready(api: &str) {
    let started = Instant::now();
    loop {
        if let Some((200, _)) = request(api, "GET", "/readyz").await {
            return;
        }
        assert!(
            started.elapsed() < READY_TIMEOUT,
            "the mail catcher at {api} did not answer /readyz within {READY_TIMEOUT:?}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn messages(api: &str) -> Vec<Value> {
    let Some((status, body)) = request(api, "GET", "/api/v1/messages?limit=200").await else {
        return Vec::new();
    };
    assert_eq!(status, 200, "{body}");
    let listed: Value =
        serde_json::from_str(&body).unwrap_or_else(|error| panic!("{error}: {body}"));
    listed["messages"].as_array().cloned().unwrap_or_default()
}

async fn wait_until<F, Fut>(what: &str, mut check: F)
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn data(reply: &Reply) -> &Value {
    reply
        .data()
        .unwrap_or_else(|| panic!("a result was expected, got {:?}", reply.error()))
}

fn subjects(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .filter_map(|message| message["Subject"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn a_raised_problem_reaches_the_mail_catcher_and_the_test_button_sends_a_test_email() {
    let catcher = catcher().await;
    let harness = Harness::new()
        .module(&keel_system::cron::MODULE)
        .module(&MODULE)
        .boot()
        .await;
    let run = Id::generate().to_string().to_lowercase();
    let subject = format!("/run/{run}");
    let channel_name = format!("catcher {run}");
    let created = harness
        .call(
            Who::Admin,
            CHANNEL_SET,
            json!({
                "kind": "email",
                "name": channel_name,
                "enabled": true,
                "email": {
                    "host": catcher.smtp_host,
                    "port": catcher.smtp_port,
                    "security": "none",
                    "from": "keel@example.org",
                    "to": "ops@example.org"
                }
            }),
        )
        .await;
    let channel: Id = data(&created)["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a ulid");
    let caller: Arc<dyn Caller> = harness
        .registry()
        .services()
        .get::<dyn Caller>()
        .expect("the kernel provides the caller");
    let problem = Problems::raise(
        caller.as_ref(),
        Raise::new("probe.disk", &subject, Severity::Warning)
            .params(json!({ "free": 5 }))
            .expect("params")
            .source("probe.disk"),
    )
    .await
    .expect("the problem is raised");
    let expected_subject = format!("Problem probe.disk raised for {subject}");
    wait_until("the raised problem reaches the catcher", || async {
        subjects(&messages(&catcher.api).await).contains(&expected_subject)
    })
    .await;
    let listed = messages(&catcher.api).await;
    let mail = listed
        .iter()
        .find(|message| message["Subject"] == json!(expected_subject))
        .expect("the mail is listed");
    assert_eq!(mail["From"]["Address"], json!("keel@example.org"));
    assert_eq!(mail["To"][0]["Address"], json!("ops@example.org"));
    let sent = harness
        .call(
            Who::Admin,
            DELIVERY_LIST,
            json!({ "filters": [{ "key": "state", "op": "eq", "value": STATE_SENT }] }),
        )
        .await;
    assert_eq!(data(&sent)["total"], json!(1));
    assert_eq!(
        data(&sent)["data"][0]["messageKey"],
        json!(KEY_PROBLEM_RAISED)
    );
    assert_eq!(data(&sent)["data"][0]["attempt"], json!(1));
    assert!(data(&sent)["data"][0]["sentAt"].is_string());
    let channel_row = harness
        .call(Who::Admin, CHANNEL_GET, json!({ "id": channel }))
        .await;
    assert!(data(&channel_row)["lastSentAt"].is_string());
    assert_eq!(data(&channel_row)["lastError"], Value::Null);
    assert_eq!(problem.subject, subject);

    let tested = harness
        .call(Who::Admin, CHANNEL_TEST, json!({ "id": channel }))
        .await;
    let message = data(&tested)["message"]
        .as_str()
        .expect("a message id")
        .to_owned();
    let test_subject = format!("Test email from the channel {channel_name}");
    wait_until("the test email reaches the catcher", || async {
        subjects(&messages(&catcher.api).await).contains(&test_subject)
    })
    .await;
    let test_mail = messages(&catcher.api)
        .await
        .into_iter()
        .find(|message| message["Subject"] == json!(test_subject))
        .expect("the test mail is listed");
    assert!(
        test_mail["Snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("test email") && snippet.contains("admin")),
        "{test_mail}"
    );
    wait_until("the test delivery is marked sent", || async {
        let listed = harness
            .call(Who::Admin, DELIVERY_LIST, json!({ "search": KEY_TEST }))
            .await;
        data(&listed)["data"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["messageId"] == json!(message) && row["state"] == json!(STATE_SENT))
        })
    })
    .await;
    harness.finish().await;
}
