use std::path::Path;
use std::time::Duration;

use keel_client::http::header::{CACHE_CONTROL, ETAG, SET_COOKIE};
use keel_client::http::{HeaderMap, HeaderName, HeaderValue, Method};
use keel_client::{Pin, Request, Response, Streaming, Trust};
use keel_types::{CallError, ErrorCode, Meta};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::report::{Failure, ensure, fail};

pub const SESSION_COOKIE: &str = keel_client::SESSION_COOKIE;
pub const SESSION_PREFIX: &str = keel_client::SESSION_PREFIX;
pub const REQUEST_ID: &str = "x-keel-request-id";
pub const META_VERSION: &str = "x-keel-meta-version";
pub const API_PREFIX: &str = keel_client::API_PREFIX;
pub const CACHE_PRIVATE: &str = "private, no-store";
pub const MASK: &str = "***";

const TIMEOUT: Duration = Duration::from_secs(30);
const STREAM_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Credential {
    Guest,
    Session,
    Cookie(String),
    Bearer(String),
}

#[derive(Clone)]
pub struct Answer {
    pub method: Method,
    pub path: String,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: String,
    pub confidential: bool,
    response: Response,
}

pub struct Client {
    keel: keel_client::Client,
    session: Option<String>,
    responses: u64,
    without_request_id: Vec<String>,
}

impl Client {
    pub fn new(url: &str, cacert: Option<&Path>) -> Result<Self, Failure> {
        let mut builder = keel_client::Client::builder(url).timeout(TIMEOUT);
        if let Some(path) = cacert {
            let pem = std::fs::read(path).map_err(|error| {
                fail(format!(
                    "--cacert {} could not be read: {error}",
                    path.display()
                ))
            })?;
            let pins = Pin::from_pem(&pem)
                .map_err(|error| fail(format!("--cacert {}: {error}", path.display())))?;
            builder = builder.trust(Trust::Pinned(pins));
        }
        let keel = builder
            .build()
            .map_err(|error| fail(format!("the client could not be built: {error}")))?;
        Ok(Self {
            keel,
            session: None,
            responses: 0,
            without_request_id: Vec::new(),
        })
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    pub fn responses(&self) -> u64 {
        self.responses
    }

    pub fn without_request_id(&self) -> &[String] {
        &self.without_request_id
    }

    pub async fn get(
        &mut self,
        path: &str,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        self.send(Method::GET, path, Request::get(path), credential, extra)
            .await
    }

    pub async fn call(
        &mut self,
        func: &str,
        params: Value,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        self.post(
            &format!("{API_PREFIX}/call/{func}"),
            params,
            credential,
            extra,
        )
        .await
    }

    pub async fn post(
        &mut self,
        path: &str,
        params: impl Serialize,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        let request = Request::post(path, &params);
        self.send(Method::POST, path, request, credential, extra)
            .await
    }

    pub async fn stream(
        &mut self,
        path: &str,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Streaming, Failure> {
        let request = self.authorize(
            Request::get(path).timeout(STREAM_TIMEOUT),
            credential,
            extra,
        )?;
        let streaming = self.keel.open(request).await?;
        let status = streaming.status();
        let headers = streaming.headers().clone();
        self.counted(&Method::GET, path, status, &headers);
        Ok(streaming)
    }

    fn authorize(
        &self,
        mut request: Request,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Request, Failure> {
        let credential = match credential {
            Credential::Guest => keel_client::Credential::guest(),
            Credential::Session => {
                let session = self
                    .session
                    .as_deref()
                    .ok_or_else(|| fail("no session cookie is held"))?;
                cookie(session)?
            }
            Credential::Cookie(pair) => cookie(&pair)?,
            Credential::Bearer(token) => {
                let bearer = if token.starts_with(keel_client::PEER_PREFIX) {
                    keel_client::Credential::peer(token)
                } else {
                    keel_client::Credential::token(token)
                };
                bearer.map_err(|error| fail(format!("the Bearer token is refused: {error}")))?
            }
        };
        request = request.credential(credential);
        for (name, value) in extra {
            request = request.header(name.clone(), value);
        }
        Ok(request)
    }

    fn counted(&mut self, method: &Method, path: &str, status: u16, headers: &HeaderMap) {
        self.responses += 1;
        if !headers.contains_key(REQUEST_ID) {
            self.without_request_id
                .push(format!("{method} {path} ({status})"));
        }
    }

    async fn send(
        &mut self,
        method: Method,
        path: &str,
        request: Request,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        let stored = matches!(credential, Credential::Guest | Credential::Session);
        let request = self.authorize(request, credential, extra)?;
        let response = self.keel.send(request).await?;
        self.counted(&method, path, response.status, &response.headers);
        let answer = Answer::new(method, path, response);
        if stored && let Some(change) = answer.session_change() {
            self.session = change;
        }
        Ok(answer)
    }
}

fn cookie(pair: &str) -> Result<keel_client::Credential, Failure> {
    let value = pair
        .strip_prefix(SESSION_COOKIE)
        .and_then(|rest| rest.strip_prefix('='))
        .ok_or_else(|| fail(format!("the cookie is not {SESSION_COOKIE}")))?;
    keel_client::Credential::session(value)
        .map_err(|error| fail(format!("the session cookie is refused: {error}")))
}

impl Answer {
    pub fn new(method: Method, path: &str, response: Response) -> Self {
        Self {
            method,
            path: path.to_owned(),
            status: response.status,
            headers: response.headers.clone(),
            body: response.text(),
            confidential: false,
            response,
        }
    }

    pub fn confidential(mut self) -> Self {
        self.confidential = true;
        self
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn request_id(&self) -> Result<&str, Failure> {
        self.header(REQUEST_ID)
            .ok_or_else(|| self.failure(format!("no {REQUEST_ID} header")))
    }

    pub fn meta_version(&self) -> Result<&str, Failure> {
        self.header(META_VERSION)
            .ok_or_else(|| self.failure(format!("no {META_VERSION} header")))
    }

    pub fn etag(&self) -> Result<&str, Failure> {
        self.header(ETAG.as_str())
            .map(|value| value.trim_matches('"'))
            .ok_or_else(|| self.failure("no ETag header"))
    }

    pub fn is_private(&self) -> Result<(), Failure> {
        ensure(
            self.header(CACHE_CONTROL.as_str()) == Some(CACHE_PRIVATE),
            || {
                self.failure(format!("Cache-Control is not {CACHE_PRIVATE}"))
                    .0
            },
        )
    }

    pub fn status_is(&self, expected: u16) -> Result<(), Failure> {
        ensure(self.status == expected, || {
            self.failure(format!("expected {expected}")).0
        })
    }

    pub fn json(&self) -> Result<Value, Failure> {
        serde_json::from_str(&self.body).map_err(|error| self.failure(format!("not JSON: {error}")))
    }

    pub fn body_as<T: DeserializeOwned>(&self) -> Result<T, Failure> {
        serde_json::from_str(&self.body)
            .map_err(|error| self.failure(format!("the body does not fit: {error}")))
    }

    pub fn data<T: DeserializeOwned>(&self) -> Result<(T, Meta), Failure> {
        self.status_is(200)?;
        match self.response.answer::<T>() {
            Ok(answer) => {
                ensure(answer.meta.request_id == self.request_id()?, || {
                    self.failure("meta.requestId differs from the header").0
                })?;
                Ok((answer.data, answer.meta))
            }
            Err(keel_client::Error::Refused(refusal)) => Err(self.failure(format!(
                "an error envelope {} {}",
                refusal.code().as_str(),
                refusal.key()
            ))),
            Err(error) => Err(self.failure(error.to_string())),
        }
    }

    pub fn error(&self, status: u16, code: ErrorCode, key: &str) -> Result<CallError, Failure> {
        self.status_is(status)?;
        match self.response.answer::<Value>() {
            Err(keel_client::Error::Refused(refusal)) => {
                let error = refusal.error;
                ensure(error.code == code, || {
                    self.failure(format!("error.code is {}", error.code.as_str()))
                        .0
                })?;
                ensure(error.key == key, || {
                    self.failure(format!("error.key is {}", error.key)).0
                })?;
                ensure(
                    error.request_id.as_deref() == Some(self.request_id()?),
                    || self.failure("error.requestId differs from the header").0,
                )?;
                Ok(error)
            }
            Ok(_) => Err(self.failure("an ok envelope")),
            Err(error) => Err(self.failure(error.to_string())),
        }
    }

    pub fn set_cookie(&self) -> Option<&str> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value: &HeaderValue| value.to_str().ok())
            .find(|value| value.starts_with(SESSION_COOKIE))
    }

    pub fn session_change(&self) -> Option<Option<String>> {
        let header = self.set_cookie()?;
        let pair = header.split(';').next().unwrap_or_default().trim();
        let value = pair.strip_prefix(SESSION_COOKIE)?.strip_prefix('=')?;
        if value.is_empty() {
            Some(None)
        } else {
            Some(Some(pair.to_owned()))
        }
    }

    pub fn session_cookie(&self) -> Result<String, Failure> {
        let header = self
            .set_cookie()
            .ok_or_else(|| self.failure("no Set-Cookie for the session"))?;
        let mut parts = header.split(';').map(str::trim);
        let pair = parts.next().unwrap_or_default();
        let value = pair
            .strip_prefix(SESSION_COOKIE)
            .and_then(|rest| rest.strip_prefix('='))
            .unwrap_or_default();
        ensure(value.starts_with(SESSION_PREFIX), || {
            self.failure(format!(
                "the cookie value does not start with {SESSION_PREFIX}"
            ))
            .0
        })?;
        let attributes: Vec<&str> = parts.collect();
        for required in ["Path=/", "Secure", "HttpOnly", "SameSite=Lax"] {
            ensure(attributes.contains(&required), || {
                self.failure(format!("the cookie lacks {required}: {}", masked(header)))
                    .0
            })?;
        }
        Ok(pair.to_owned())
    }

    pub fn clears_session(&self) -> Result<(), Failure> {
        let header = self
            .set_cookie()
            .ok_or_else(|| self.failure("no Set-Cookie clearing the session"))?;
        ensure(
            self.session_change() == Some(None) && header.contains("Max-Age=0"),
            || {
                self.failure(format!("the cookie is not cleared: {}", masked(header)))
                    .0
            },
        )
    }

    pub fn failure(&self, message: impl Into<String>) -> Failure {
        let mut line = format!(
            "{} {} answered {}: {}",
            self.method,
            self.path,
            self.status,
            message.into()
        );
        if !self.confidential && !self.body.is_empty() {
            line.push_str(": ");
            line.extend(self.body.chars().take(300));
        }
        fail(line)
    }
}

pub fn masked(set_cookie: &str) -> String {
    let mut parts = set_cookie.split(';').map(str::trim);
    let pair = parts.next().unwrap_or_default();
    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
    let shown = if value.is_empty() { "" } else { MASK };
    let mut line = format!("{name}={shown}");
    for attribute in parts {
        line.push_str("; ");
        line.push_str(attribute);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(headers: &[(&'static str, &str)], body: &str) -> Answer {
        answer_with(200, headers, body)
    }

    fn answer_with(status: u16, headers: &[(&'static str, &str)], body: &str) -> Answer {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let response = Response {
            url: "https://127.0.0.1:1500/api/v1/call/x".into(),
            status,
            headers: map,
            body: body.as_bytes().to_vec(),
        };
        Answer::new(Method::POST, "/api/v1/call/x", response)
    }

    #[test]
    fn a_failure_line_masks_the_cookie_value_and_omits_the_body_of_a_confidential_answer() {
        let lax = answer(
            &[(
                "set-cookie",
                "__Host-keel_session=ks_secret; Path=/; Secure",
            )],
            "",
        );
        let line = lax.session_cookie().unwrap_err().0;
        assert!(
            line.contains("__Host-keel_session=***; Path=/; Secure"),
            "{line}"
        );
        assert!(!line.contains("ks_secret"), "{line}");
        let kept = answer(
            &[("set-cookie", "__Host-keel_session=ks_secret; Path=/")],
            "",
        );
        let line = kept.clears_session().unwrap_err().0;
        assert!(line.ends_with("__Host-keel_session=***; Path=/"), "{line}");
        assert!(!line.contains("ks_secret"), "{line}");
        assert_eq!(
            masked("__Host-keel_session=; Path=/; Max-Age=0"),
            "__Host-keel_session=; Path=/; Max-Age=0"
        );

        let minted = r#"{"ok":true,"data":{"token":"kat_secret"},"meta":{"requestId":"r"}}"#;
        let plain = answer(&[("x-keel-request-id", "r")], minted);
        assert!(plain.failure("no data.id").0.ends_with(minted));
        let confidential = plain.clone().confidential();
        assert_eq!(
            confidential.failure("no data.id").0,
            "POST /api/v1/call/x answered 200: no data.id"
        );
        let line = confidential.data::<Vec<u8>>().unwrap_err().0;
        assert!(!line.contains("kat_secret"), "{line}");
    }

    #[test]
    fn the_session_cookie_is_read_with_its_attributes_and_a_cleared_one_is_told_apart() {
        let set = answer(
            &[(
                "set-cookie",
                "__Host-keel_session=ks_abc; Path=/; Secure; HttpOnly; SameSite=Lax",
            )],
            "",
        );
        assert_eq!(
            set.session_cookie().unwrap(),
            "__Host-keel_session=ks_abc".to_owned()
        );
        assert_eq!(
            set.session_change(),
            Some(Some("__Host-keel_session=ks_abc".to_owned()))
        );
        let cleared = answer(
            &[(
                "set-cookie",
                "__Host-keel_session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0",
            )],
            "",
        );
        assert_eq!(cleared.session_change(), Some(None));
        assert!(cleared.clears_session().is_ok());
        assert!(cleared.session_cookie().is_err());
        let lax = answer(
            &[("set-cookie", "__Host-keel_session=ks_abc; Path=/; Secure")],
            "",
        );
        assert!(lax.session_cookie().is_err());
        assert_eq!(answer(&[], "").session_change(), None);
    }

    #[test]
    fn an_envelope_is_checked_against_its_status_code_key_and_request_id() {
        let ok = answer(
            &[("x-keel-request-id", "req-1")],
            r#"{"ok":true,"data":{"a":1},"meta":{"requestId":"req-1"}}"#,
        );
        let (data, meta) = ok.data::<Value>().unwrap();
        assert_eq!(data["a"], 1);
        assert_eq!(meta.request_id, "req-1");
        let mismatched = answer(
            &[("x-keel-request-id", "req-2")],
            r#"{"ok":true,"data":{"a":1},"meta":{"requestId":"req-1"}}"#,
        );
        assert!(mismatched.data::<Value>().is_err());
        let refused = answer_with(
            404,
            &[("x-keel-request-id", "req-3")],
            r#"{"ok":false,"error":{"code":"not_found","key":"keel.error.function_not_found","requestId":"req-3"}}"#,
        );
        assert!(
            refused
                .error(404, ErrorCode::NotFound, "keel.error.function_not_found")
                .is_ok()
        );
        assert!(
            refused
                .error(404, ErrorCode::NotFound, "keel.error.not_found")
                .is_err()
        );
        assert!(
            refused
                .error(403, ErrorCode::NotFound, "keel.error.function_not_found")
                .is_err()
        );
    }
}
