use std::future::IntoFuture;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::http::header::AUTHORIZATION;
use axum::middleware::{self, Next};
use axum::response::Response;
use keel_kernel::http::{LAST_EVENT_ID, SESSION_COOKIE};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::auth::PRINCIPAL_HEADER;
use crate::events::EventStream;
use crate::fixtures::Who;

pub const EVENTS_PATH: &str = "/api/v1/events";
pub const UPLOAD_BOUNDARY: &str = "keel-testkit-boundary-7f3a2c";

const HOST: &str = "keel.test";
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadPart<'a> {
    pub field: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

impl<'a> UploadPart<'a> {
    pub fn new(field: &'a str, filename: &'a str, content_type: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            field,
            filename,
            content_type,
            bytes,
        }
    }
}

pub fn multipart_body(params: &Value, files: &[UploadPart<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{UPLOAD_BOUNDARY}\r\nContent-Disposition: form-data; name=\"params\"\r\nContent-Type: application/json\r\n\r\n{params}\r\n"
        )
        .as_bytes(),
    );
    for part in files {
        body.extend_from_slice(
            format!(
                "--{UPLOAD_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
                part.field, part.filename, part.content_type
            )
            .as_bytes(),
        );
        body.extend_from_slice(part.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{UPLOAD_BOUNDARY}--\r\n").as_bytes());
    body
}

pub struct TestHttp {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("could not bind a local port: {0}")]
    Bind(#[source] io::Error),
    #[error("the request to {path} failed: {source}")]
    Request {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("the answer to {path} is not HTTP: {reason}")]
    Malformed { path: String, reason: String },
    #[error("the answer to {path} did not arrive within {READ_TIMEOUT:?}")]
    Timeout { path: String },
    #[error("stopping the server failed: {0}")]
    Stop(#[source] io::Error),
}

pub fn with_test_principal(router: Router) -> Router {
    router.layer(middleware::from_fn(map_principal_header))
}

async fn map_principal_header(mut request: Request, next: Next) -> Response {
    let login = request
        .headers()
        .get(PRINCIPAL_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|login| !login.is_empty() && *login != Who::Guest.login())
        .map(str::to_owned);
    if let Some(login) = login
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {login}"))
    {
        request.headers_mut().insert(AUTHORIZATION, value);
    }
    next.run(request).await
}

impl TestHttp {
    pub async fn bind(router: Router) -> Result<Self, HttpError> {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .map_err(HttpError::Bind)?;
        let addr = listener.local_addr().map_err(HttpError::Bind)?;
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
            })
            .into_future(),
        );
        Ok(Self {
            addr,
            stop: Some(stop),
            task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn get(&self, who: Who, path: &str) -> Answer {
        self.request("GET", path, &[(PRINCIPAL_HEADER, who.login())], None)
            .await
    }

    pub async fn events(&self, who: Who, last_event_id: Option<u64>) -> EventStream {
        let id = last_event_id.map(|id| id.to_string());
        let header = LAST_EVENT_ID;
        let mut headers = vec![(PRINCIPAL_HEADER, who.login())];
        if let Some(id) = &id {
            headers.push((header.as_str(), id.as_str()));
        }
        self.stream(EVENTS_PATH, &headers).await
    }

    pub async fn stream(&self, path: &str, headers: &[(&str, &str)]) -> EventStream {
        match EventStream::open(self.addr, path, headers).await {
            Ok(stream) => stream,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn post(&self, who: Who, path: &str, body: &Value) -> Answer {
        self.request(
            "POST",
            path,
            &[
                (PRINCIPAL_HEADER, who.login()),
                ("content-type", "application/json"),
            ],
            Some(&body.to_string()),
        )
        .await
    }

    pub async fn upload(
        &self,
        who: Who,
        path: &str,
        params: &Value,
        files: &[UploadPart<'_>],
    ) -> Answer {
        self.upload_with(who, path, &[], params, files).await
    }

    pub async fn upload_with(
        &self,
        who: Who,
        path: &str,
        headers: &[(&str, &str)],
        params: &Value,
        files: &[UploadPart<'_>],
    ) -> Answer {
        let content_type = format!("multipart/form-data; boundary={UPLOAD_BOUNDARY}");
        let mut all = vec![
            (PRINCIPAL_HEADER, who.login()),
            ("content-type", content_type.as_str()),
        ];
        all.extend_from_slice(headers);
        let body = multipart_body(params, files);
        match self.try_request_bytes("POST", path, &all, &body).await {
            Ok(answer) => answer,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Answer {
        match self.try_request(method, path, headers, body).await {
            Ok(answer) => answer,
            Err(error) => panic!("{error}"),
        }
    }

    pub async fn try_request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<Answer, HttpError> {
        self.try_request_bytes(method, path, headers, body.unwrap_or_default().as_bytes())
            .await
    }

    pub async fn try_request_bytes(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<Answer, HttpError> {
        let failed = |source| HttpError::Request {
            path: path.to_owned(),
            source,
        };
        let host = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("host"))
            .map_or(HOST, |(_, value)| *value);
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        for (name, value) in headers
            .iter()
            .filter(|(name, _)| !name.eq_ignore_ascii_case("host"))
        {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut request = request.into_bytes();
        request.extend_from_slice(body);
        let exchange = async {
            let mut stream = TcpStream::connect(self.addr).await?;
            stream.write_all(&request).await?;
            let mut raw = Vec::new();
            stream.read_to_end(&mut raw).await?;
            Ok::<Vec<u8>, io::Error>(raw)
        };
        let raw = tokio::time::timeout(READ_TIMEOUT, exchange)
            .await
            .map_err(|_| HttpError::Timeout {
                path: path.to_owned(),
            })?
            .map_err(failed)?;
        parse(&raw).map_err(|reason| HttpError::Malformed {
            path: path.to_owned(),
            reason,
        })
    }

    pub async fn stop(mut self) -> Result<(), HttpError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.await {
            Ok(outcome) => outcome.map_err(HttpError::Stop),
            Err(error) => Err(HttpError::Stop(io::Error::other(error))),
        }
    }
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> Value {
        match serde_json::from_str(&self.body) {
            Ok(value) => value,
            Err(error) => panic!("the body is not JSON ({error}): {}", self.body),
        }
    }

    pub fn set_cookie(&self) -> Option<&str> {
        self.header("set-cookie")
    }

    pub fn session_cookie(&self) -> Option<String> {
        let value = self
            .set_cookie()?
            .split(';')
            .next()?
            .trim()
            .strip_prefix(SESSION_COOKIE)?
            .strip_prefix('=')?
            .trim();
        (!value.is_empty()).then(|| format!("{SESSION_COOKIE}={value}"))
    }
}

fn parse(raw: &[u8]) -> Result<Answer, String> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "no end of headers".to_owned())?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| "no status line".to_owned())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let rest = &raw[split + 4..];
    let chunked = headers.iter().any(|(name, value)| {
        name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked")
    });
    let body = if chunked {
        dechunk(rest)?
    } else {
        rest.to_vec()
    };
    Ok(Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn dechunk(mut rest: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let line_end = rest
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "unterminated chunk size".to_owned())?;
        let size_text = String::from_utf8_lossy(&rest[..line_end]);
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let chunk = rest
            .get(..size)
            .ok_or_else(|| "truncated chunk".to_owned())?;
        body.extend_from_slice(chunk);
        rest = rest
            .get(size + 2..)
            .ok_or_else(|| "chunk without its terminator".to_owned())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_answer_is_split_into_status_headers_and_body() {
        let answer = parse(
            b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nX-Keel-Request-Id: req-1\r\n\r\n{\"ok\":false}",
        )
        .unwrap();
        assert_eq!(answer.status, 404);
        assert_eq!(answer.header("x-keel-request-id"), Some("req-1"));
        assert_eq!(answer.header("Content-Type"), Some("application/json"));
        assert_eq!(answer.json()["ok"], Value::Bool(false));
        assert_eq!(answer.session_cookie(), None);
    }

    #[test]
    fn the_session_cookie_is_read_back_from_set_cookie_as_a_cookie_header_value() {
        let set = parse(
            b"HTTP/1.1 200 OK\r\nSet-Cookie: __Host-keel_session=ks_1; Path=/; Secure; HttpOnly; SameSite=Lax\r\n\r\n{}",
        )
        .unwrap();
        assert_eq!(
            set.session_cookie().as_deref(),
            Some("__Host-keel_session=ks_1")
        );
        let cleared = parse(
            b"HTTP/1.1 200 OK\r\nSet-Cookie: __Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0\r\n\r\n{}",
        )
        .unwrap();
        assert_eq!(cleared.session_cookie(), None);
        assert!(cleared.set_cookie().unwrap().ends_with("Max-Age=0"));
    }

    #[test]
    fn a_multipart_body_carries_the_params_part_first_and_one_part_per_file() {
        let body = multipart_body(
            &serde_json::json!({ "host": "panel.example" }),
            &[UploadPart::new(
                "logo",
                "logo.svg",
                "image/svg+xml",
                b"<svg/>",
            )],
        );
        let text = String::from_utf8(body).unwrap();
        assert_eq!(
            text,
            format!(
                "--{UPLOAD_BOUNDARY}\r\nContent-Disposition: form-data; name=\"params\"\r\nContent-Type: application/json\r\n\r\n{{\"host\":\"panel.example\"}}\r\n--{UPLOAD_BOUNDARY}\r\nContent-Disposition: form-data; name=\"logo\"; filename=\"logo.svg\"\r\nContent-Type: image/svg+xml\r\n\r\n<svg/>\r\n--{UPLOAD_BOUNDARY}--\r\n"
            )
        );
    }

    #[test]
    fn a_chunked_answer_is_reassembled() {
        let answer = parse(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;ext\r\n:1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body, "{\"a\":1}");
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab").is_err());
        assert!(parse(b"garbage").is_err());
    }
}
