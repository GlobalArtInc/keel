use std::path::Path;
use std::time::Duration;

use keel_types::{CallError, Envelope, ErrorCode, Meta};
use reqwest::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, ETAG, HeaderMap, HeaderName, HeaderValue,
    SET_COOKIE,
};
use reqwest::{Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::report::{Failure, ensure, fail};

pub const SESSION_COOKIE: &str = "__Host-keel_session";
pub const SESSION_PREFIX: &str = "ks_";
pub const REQUEST_ID: &str = "x-keel-request-id";
pub const META_VERSION: &str = "x-keel-meta-version";
pub const API_PREFIX: &str = "/api/v1";
pub const CACHE_PRIVATE: &str = "private, no-store";
pub const MASK: &str = "***";

const TIMEOUT: Duration = Duration::from_secs(30);

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
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    session: Option<String>,
    responses: u64,
    without_request_id: Vec<String>,
}

impl Client {
    pub fn new(url: &str, cacert: Option<&Path>) -> Result<Self, Failure> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = reqwest::Client::builder().no_proxy().timeout(TIMEOUT);
        if let Some(path) = cacert {
            let pem = std::fs::read(path).map_err(|error| {
                fail(format!(
                    "--cacert {} could not be read: {error}",
                    path.display()
                ))
            })?;
            let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|error| {
                fail(format!(
                    "--cacert {} is not a PEM certificate: {error}",
                    path.display()
                ))
            })?;
            if certificates.is_empty() {
                return Err(fail(format!(
                    "--cacert {} holds no CERTIFICATE block",
                    path.display()
                )));
            }
            builder = builder.tls_certs_only(certificates);
        }
        let http = builder
            .build()
            .map_err(|error| fail(format!("the HTTP client could not be built: {error}")))?;
        Ok(Self {
            http,
            base: url.trim_end_matches('/').to_owned(),
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
        let request = self.http.get(format!("{}{path}", self.base));
        self.send(Method::GET, path, request, credential, extra)
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
        params: Value,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        let request = self
            .http
            .post(format!("{}{path}", self.base))
            .header(CONTENT_TYPE, "application/json")
            .body(params.to_string());
        self.send(Method::POST, path, request, credential, extra)
            .await
    }

    async fn send(
        &mut self,
        method: Method,
        path: &str,
        mut request: RequestBuilder,
        credential: Credential,
        extra: &[(HeaderName, &str)],
    ) -> Result<Answer, Failure> {
        let stored = matches!(credential, Credential::Guest | Credential::Session);
        match credential {
            Credential::Guest => {}
            Credential::Session => {
                let session = self
                    .session
                    .as_deref()
                    .ok_or_else(|| fail("no session cookie is held"))?;
                request = request.header(COOKIE, session);
            }
            Credential::Cookie(cookie) => request = request.header(COOKIE, cookie),
            Credential::Bearer(token) => {
                request = request.header(AUTHORIZATION, format!("Bearer {token}"));
            }
        }
        for (name, value) in extra {
            request = request.header(name.clone(), *value);
        }
        let response = request.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.text().await?;
        let answer = Answer {
            method,
            path: path.to_owned(),
            status,
            headers,
            body,
            confidential: false,
        };
        self.responses += 1;
        if answer.header(REQUEST_ID).is_none() {
            self.without_request_id.push(format!(
                "{} {} ({})",
                answer.method, answer.path, answer.status
            ));
        }
        if stored && let Some(change) = answer.session_change() {
            self.session = change;
        }
        Ok(answer)
    }
}

impl Answer {
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
        match self.body_as::<Envelope<T>>()? {
            Envelope::Ok { data, meta } => {
                ensure(meta.request_id == self.request_id()?, || {
                    self.failure("meta.requestId differs from the header").0
                })?;
                Ok((data, meta))
            }
            Envelope::Err { error } => Err(self.failure(format!(
                "an error envelope {} {}",
                error.code.as_str(),
                error.key
            ))),
        }
    }

    pub fn error(&self, status: u16, code: ErrorCode, key: &str) -> Result<CallError, Failure> {
        self.status_is(status)?;
        match self.body_as::<Envelope<Value>>()? {
            Envelope::Err { error } => {
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
            Envelope::Ok { .. } => Err(self.failure("an ok envelope")),
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
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        Answer {
            method: Method::POST,
            path: "/api/v1/call/x".into(),
            status: 200,
            headers: map,
            body: body.into(),
            confidential: false,
        }
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
        let mut refused = answer(
            &[("x-keel-request-id", "req-3")],
            r#"{"ok":false,"error":{"code":"not_found","key":"keel.error.function_not_found","requestId":"req-3"}}"#,
        );
        refused.status = 404;
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
