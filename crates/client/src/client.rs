use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::header::{ACCEPT, CONTENT_TYPE, SET_COOKIE};
use http::{HeaderMap, HeaderName, HeaderValue, Method};
use keel_types::InstanceInfo;
use reqwest::Url;
use reqwest::redirect::Policy;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::call::{Answer, Call};
use crate::credential::{Credential, OnBehalf, apply_key};
use crate::error::Error;
use crate::feed::Feed;
use crate::tls::{self, Trust};
use crate::{
    ANSWER_LIMIT, ATTEMPTS, BACKOFF, BACKOFF_MAX, CONNECT_TIMEOUT, IDEMPOTENT_REPLAY,
    INSTANCE_PATH, SESSION_COOKIE, TIMEOUT, VERSION,
};

const JSON: &str = "application/json";
const REASON_CAP: usize = 512;

pub struct Builder {
    url: String,
    trust: Trust,
    credential: Credential,
    on_behalf: Option<OnBehalf>,
    timeout: Duration,
    connect_timeout: Duration,
    attempts: u32,
    backoff: Duration,
    answer_limit: usize,
}

impl Builder {
    pub fn trust(mut self, trust: Trust) -> Self {
        self.trust = trust;
        self
    }

    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = credential;
        self
    }

    pub fn on_behalf(mut self, on_behalf: OnBehalf) -> Self {
        self.on_behalf = Some(on_behalf);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn attempts(mut self, attempts: u32) -> Self {
        self.attempts = attempts.max(1);
        self
    }

    pub fn backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn answer_limit(mut self, bytes: usize) -> Self {
        self.answer_limit = bytes;
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        let base = base_of(&self.url)?;
        let secure = base.starts_with("https://");
        if !secure && matches!(self.trust, Trust::Pinned(_)) {
            return Err(Error::invalid(
                "a certificate is pinned for an http base, which presents none",
            ));
        }
        let config = tls::config(secure.then_some(&self.trust))?;
        let http = reqwest::Client::builder()
            .user_agent(format!("keel-client/{VERSION}"))
            .connect_timeout(self.connect_timeout)
            .timeout(self.timeout)
            .redirect(Policy::none())
            .no_proxy()
            .tls_backend_preconfigured(config)
            .build()
            .map_err(|error| Error::invalid(format!("the HTTP client cannot be built: {error}")))?;
        Ok(Client {
            shared: Arc::new(Shared {
                http,
                base,
                attempts: self.attempts,
                backoff: self.backoff,
                answer_limit: self.answer_limit,
            }),
            credential: self.credential,
            on_behalf: self.on_behalf,
        })
    }
}

struct Shared {
    http: reqwest::Client,
    base: String,
    attempts: u32,
    backoff: Duration,
    answer_limit: usize,
}

#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    credential: Credential,
    on_behalf: Option<OnBehalf>,
}

#[derive(Clone)]
pub struct Request {
    method: Method,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(HeaderName, String)>,
    body: Option<Result<Vec<u8>, String>>,
    credential: Option<Credential>,
    on_behalf: Option<OnBehalf>,
    idempotency_key: Option<String>,
    timeout: Option<Duration>,
}

#[derive(Clone)]
pub struct Response {
    pub url: String,
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub struct Streaming {
    url: String,
    response: reqwest::Response,
}

struct Prepared {
    url: Url,
    method: Method,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
    timeout: Option<Duration>,
}

impl Client {
    pub fn builder(url: &str) -> Builder {
        Builder {
            url: url.to_owned(),
            trust: Trust::System,
            credential: Credential::guest(),
            on_behalf: None,
            timeout: TIMEOUT,
            connect_timeout: CONNECT_TIMEOUT,
            attempts: ATTEMPTS,
            backoff: BACKOFF,
            answer_limit: ANSWER_LIMIT,
        }
    }

    pub fn new(url: &str) -> Result<Self, Error> {
        Self::builder(url).build()
    }

    pub fn base(&self) -> &str {
        &self.shared.base
    }

    pub fn with_credential(&self, credential: Credential) -> Self {
        Self {
            shared: self.shared.clone(),
            credential,
            on_behalf: self.on_behalf.clone(),
        }
    }

    pub fn with_on_behalf(&self, on_behalf: Option<OnBehalf>) -> Self {
        Self {
            shared: self.shared.clone(),
            credential: self.credential.clone(),
            on_behalf,
        }
    }

    pub fn call<T: DeserializeOwned>(&self, func: &str, params: impl Serialize) -> Call<'_, T> {
        Call::new(self, func, params)
    }

    pub async fn instance(&self) -> Result<InstanceInfo, Error> {
        let request = Request::get(INSTANCE_PATH)
            .credential(Credential::guest())
            .header(ACCEPT, JSON);
        let response = self.send_as(request, None).await?;
        response.json::<InstanceInfo>()
    }

    pub fn feed(&self, after: u64) -> Feed {
        Feed::new(self.clone(), after)
    }

    pub async fn send(&self, request: Request) -> Result<Response, Error> {
        let on_behalf = request.on_behalf.clone().or_else(|| self.on_behalf.clone());
        self.send_as(request, on_behalf).await
    }

    pub async fn open(&self, request: Request) -> Result<Streaming, Error> {
        let on_behalf = request.on_behalf.clone().or_else(|| self.on_behalf.clone());
        let prepared = self.prepare(&request, on_behalf.as_ref())?;
        let url = prepared.url.to_string();
        let response = self
            .outgoing(&prepared)
            .send()
            .await
            .map_err(|error| transport(&url, &error))?;
        Ok(Streaming { url, response })
    }

    pub(crate) async fn send_as(
        &self,
        request: Request,
        on_behalf: Option<OnBehalf>,
    ) -> Result<Response, Error> {
        let prepared = self.prepare(&request, on_behalf.as_ref())?;
        let attempts = if request.idempotency_key.is_some() {
            self.shared.attempts
        } else {
            1
        };
        let mut attempt = 1;
        loop {
            let outcome = self.attempt(&prepared).await;
            let failed = match &outcome {
                Ok(response) => response.status >= 500,
                Err(error) => error.is_transport(),
            };
            if !failed || attempt >= attempts {
                return outcome.map_err(|error| error.tried(attempt));
            }
            tokio::time::sleep(delay(self.shared.backoff, attempt)).await;
            attempt += 1;
        }
    }

    fn prepare(&self, request: &Request, on_behalf: Option<&OnBehalf>) -> Result<Prepared, Error> {
        if !request.path.starts_with('/') || request.path.starts_with("//") {
            return Err(Error::invalid(format!(
                "the path {:?} does not start with a single /",
                request.path
            )));
        }
        let mut url = Url::parse(&format!("{}{}", self.shared.base, request.path))
            .map_err(|error| Error::invalid(format!("the address is not a URL: {error}")))?;
        if !request.query.is_empty() {
            url.query_pairs_mut().extend_pairs(&request.query);
        }
        let mut headers = HeaderMap::new();
        request
            .credential
            .as_ref()
            .unwrap_or(&self.credential)
            .apply(&mut headers)?;
        if let Some(on_behalf) = on_behalf {
            on_behalf.apply(&mut headers)?;
        }
        if let Some(key) = &request.idempotency_key {
            apply_key(key, &mut headers)?;
        }
        for (name, value) in &request.headers {
            let value = HeaderValue::from_str(value).map_err(|_| {
                Error::invalid(format!("the value of {name} is not a header value"))
            })?;
            headers.insert(name.clone(), value);
        }
        let body = match &request.body {
            None => None,
            Some(Ok(body)) => Some(body.clone()),
            Some(Err(reason)) => {
                return Err(Error::invalid(format!(
                    "the parameters are not JSON: {reason}"
                )));
            }
        };
        Ok(Prepared {
            url,
            method: request.method.clone(),
            headers,
            body,
            timeout: request.timeout,
        })
    }

    fn outgoing(&self, prepared: &Prepared) -> reqwest::RequestBuilder {
        let mut builder = self
            .shared
            .http
            .request(prepared.method.clone(), prepared.url.clone())
            .headers(prepared.headers.clone());
        if let Some(body) = &prepared.body {
            builder = builder.body(body.clone());
        }
        if let Some(timeout) = prepared.timeout {
            builder = builder.timeout(timeout);
        }
        builder
    }

    async fn attempt(&self, prepared: &Prepared) -> Result<Response, Error> {
        let url = prepared.url.to_string();
        let mut response = self
            .outgoing(prepared)
            .send()
            .await
            .map_err(|error| transport(&url, &error))?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let limit = self.shared.answer_limit;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| transport(&url, &error))?
        {
            if body.len() + chunk.len() > limit {
                return Err(Error::Answer {
                    url,
                    status,
                    reason: format!("the answer is larger than {limit} bytes"),
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response {
            url,
            status,
            headers,
            body,
        })
    }
}

impl Request {
    fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            credential: None,
            on_behalf: None,
            idempotency_key: None,
            timeout: None,
        }
    }

    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    pub fn post(path: &str, body: &impl Serialize) -> Self {
        let mut request = Self::new(Method::POST, path).header(CONTENT_TYPE, JSON);
        request.body = Some(serde_json::to_vec(body).map_err(|error| error.to_string()));
        request
    }

    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn header(mut self, name: HeaderName, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    pub fn credential(mut self, credential: Credential) -> Self {
        self.credential = Some(credential);
        self
    }

    pub fn on_behalf(mut self, on_behalf: OnBehalf) -> Self {
        self.on_behalf = Some(on_behalf);
        self
    }

    pub fn idempotency_key(mut self, key: &str) -> Self {
        self.idempotency_key = Some(key.to_owned());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("headers", &headers)
            .field(
                "body",
                &self.body.as_ref().map(|body| body.as_ref().map(Vec::len)),
            )
            .field("credential", &self.credential)
            .field("on_behalf", &self.on_behalf)
            .field("idempotency_key", &self.idempotency_key)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<&str> = self.headers.keys().map(|name| name.as_str()).collect();
        f.debug_struct("Response")
            .field("url", &self.url)
            .field("status", &self.status)
            .field("headers", &headers)
            .field("body", &self.body.len())
            .finish()
    }
}

impl Response {
    pub fn header(&self, name: impl http::header::AsHeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn replayed(&self) -> bool {
        self.header(IDEMPOTENT_REPLAY) == Some("true")
    }

    pub fn session(&self) -> Option<Credential> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|cookie| cookie.split(';').next())
            .filter_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('='))
            .find_map(|value| Credential::session(value).ok())
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        if self.status != 200 {
            return Err(match self.answer::<serde_json::Value>() {
                Err(error) => error,
                Ok(_) => self.outside("an ok envelope under another status"),
            });
        }
        serde_json::from_slice(&self.body)
            .map_err(|error| self.outside(&format!("the body does not fit: {}", shape(&error))))
    }

    pub fn answer<T: DeserializeOwned>(&self) -> Result<Answer<T>, Error> {
        Answer::read(self)
    }

    pub(crate) fn outside(&self, reason: &str) -> Error {
        Error::Answer {
            url: self.url.clone(),
            status: self.status,
            reason: reason.to_owned(),
        }
    }
}

impl Streaming {
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn status(&self) -> u16 {
        self.response.status().as_u16()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    pub async fn chunk(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.response
            .chunk()
            .await
            .map(|chunk| chunk.map(|bytes| bytes.to_vec()))
            .map_err(|error| transport(&self.url, &error))
    }
}

pub(crate) fn shape(error: &serde_json::Error) -> String {
    format!(
        "{:?} at line {} column {}",
        error.classify(),
        error.line(),
        error.column()
    )
}

pub(crate) fn delay(backoff: Duration, attempt: u32) -> Duration {
    let factor = 1u32 << attempt.saturating_sub(1).min(16);
    backoff.saturating_mul(factor).min(BACKOFF_MAX)
}

fn base_of(url: &str) -> Result<String, Error> {
    let base = url.trim().trim_end_matches('/');
    let refused = || {
        Error::invalid(
            "the base is not an http or https address without credentials, query or fragment",
        )
    };
    let parsed = Url::parse(base).map_err(|_| refused())?;
    let plain = matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none();
    if plain {
        Ok(parsed.as_str().trim_end_matches('/').to_owned())
    } else {
        Err(refused())
    }
}

fn transport(url: &str, error: &reqwest::Error) -> Error {
    let mut reason = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        let line = inner.to_string();
        if !reason.contains(&line) {
            reason.push_str(": ");
            reason.push_str(&line);
        }
        source = inner.source();
    }
    if reason.len() > REASON_CAP {
        let mut end = REASON_CAP;
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        reason.truncate(end);
    }
    Error::Transport {
        url: url.to_owned(),
        attempts: 1,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use crate::tls::Pin;

    use super::*;

    #[test]
    fn the_base_is_an_http_or_https_address_without_credentials_query_or_fragment() {
        assert_eq!(
            base_of(" https://dns.example:8443/ ").unwrap(),
            "https://dns.example:8443"
        );
        assert_eq!(
            base_of("http://10.0.0.5/keel/").unwrap(),
            "http://10.0.0.5/keel"
        );
        assert_eq!(
            base_of("HTTPS://DNS.Example:8443/Keel/").unwrap(),
            "https://dns.example:8443/Keel"
        );
        assert_eq!(base_of("Http://dns.example").unwrap(), "http://dns.example");
        assert_eq!(
            base_of("https://dns.example:443").unwrap(),
            "https://dns.example"
        );
        for refused in [
            "",
            "dns.example",
            "ftp://dns.example",
            "https://user:secret@dns.example",
            "https://dns.example/?a=b",
            "https://dns.example/#top",
            "file:///etc/keel",
            "FTP://dns.example",
        ] {
            assert!(
                matches!(base_of(refused), Err(Error::Invalid(_))),
                "{refused}"
            );
        }
    }

    #[test]
    fn a_pinned_trust_needs_an_https_base_whatever_the_case_of_its_scheme() {
        let pin = Pin::of(b"the certificate of the installation");
        let pinned = |base: &str, pins: Vec<Pin>| {
            Client::builder(base)
                .trust(Trust::Pinned(pins))
                .credential(Credential::peer("kpt_0a1b").unwrap())
                .build()
        };
        let secure = pinned("HTTPS://DNS.Example:8443/", vec![pin]).unwrap();
        assert_eq!(secure.base(), "https://dns.example:8443");
        for (base, pins) in [
            ("http://dns.example", vec![pin]),
            ("HTTP://dns.example", vec![pin]),
            ("http://dns.example", Vec::new()),
            ("HTTPS://dns.example", Vec::new()),
        ] {
            assert!(
                matches!(pinned(base, pins.clone()), Err(Error::Invalid(_))),
                "{base} {pins:?}"
            );
        }
        let plain = Client::builder("Http://dns.example")
            .trust(Trust::System)
            .build()
            .unwrap();
        assert_eq!(plain.base(), "http://dns.example");
    }

    #[test]
    fn the_wait_between_attempts_doubles_from_the_backoff_up_to_its_ceiling() {
        let backoff = Duration::from_millis(250);
        assert_eq!(delay(backoff, 1), Duration::from_millis(250));
        assert_eq!(delay(backoff, 2), Duration::from_millis(500));
        assert_eq!(delay(backoff, 3), Duration::from_secs(1));
        assert_eq!(delay(backoff, 6), BACKOFF_MAX);
        assert_eq!(delay(backoff, u32::MAX), BACKOFF_MAX);
        assert_eq!(delay(Duration::ZERO, 4), Duration::ZERO);
    }

    #[test]
    fn a_request_names_a_path_under_the_base_with_its_credential_on_behalf_and_key() {
        let client = Client::builder("http://127.0.0.1:1/keel")
            .credential(Credential::token("kat_0a1b").unwrap())
            .build()
            .unwrap();
        let customer = OnBehalf::new("bill:01J9A00000000000000000000A", "client-42").unwrap();
        let request = Request::post("/api/v1/call/note.edit", &serde_json::json!({ "a": 1 }))
            .query("types", "dns.zone.*,svc.*")
            .idempotency_key("bill-1")
            .header(ACCEPT, JSON);
        let prepared = client.prepare(&request, Some(&customer)).unwrap();
        assert_eq!(
            prepared.url.as_str(),
            "http://127.0.0.1:1/keel/api/v1/call/note.edit?types=dns.zone.*%2Csvc.*"
        );
        assert_eq!(prepared.headers["authorization"], "Bearer kat_0a1b");
        assert_eq!(
            prepared.headers["x-keel-on-behalf-of"],
            "origin=bill:01J9A00000000000000000000A; external_id=client-42"
        );
        assert_eq!(prepared.headers["idempotency-key"], "bill-1");
        assert_eq!(prepared.headers["content-type"], JSON);
        assert_eq!(prepared.body.as_deref(), Some(&b"{\"a\":1}"[..]));
        let session = Credential::session("ks_0a1b").unwrap();
        let guest = client
            .prepare(&Request::get("/healthz").credential(session), None)
            .unwrap();
        assert_eq!(guest.headers["cookie"], "__Host-keel_session=ks_0a1b");
        assert!(!guest.headers.contains_key("authorization"));
        for refused in [
            Request::get("healthz"),
            Request::get("//evil.example/x"),
            Request::get("/x").idempotency_key(" padded"),
            Request::get("/x").header(ACCEPT, "a\r\nb"),
            Request::post("/x", &std::collections::BTreeMap::from([(vec![1u8], 1)])),
        ] {
            assert!(
                matches!(client.prepare(&refused, None), Err(Error::Invalid(_))),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn a_request_or_a_response_never_prints_its_body_or_its_header_values() {
        let request = Request::post(
            "/api/v1/call/auth.login",
            &serde_json::json!({ "login": "root", "password": "hunter2-secret" }),
        )
        .header(http::header::AUTHORIZATION, "Bearer kat_secret")
        .credential(Credential::session("ks_secret").unwrap());
        let printed = format!("{request:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(printed.contains("authorization"), "{printed}");
        let mut headers = HeaderMap::new();
        headers.insert(
            SET_COOKIE,
            HeaderValue::from_static("__Host-keel_session=ks_secret"),
        );
        let response = Response {
            url: "https://dns.example/api/v1/call/auth.token.create".to_owned(),
            status: 200,
            headers,
            body: br#"{"ok":true,"data":{"token":"kat_secret"},"meta":{"requestId":"r"}}"#.to_vec(),
        };
        let printed = format!("{response:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(printed.contains("set-cookie"), "{printed}");
    }

    #[test]
    fn the_session_a_login_sets_is_read_from_its_cookie() {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_static("other=1; Path=/"));
        headers.append(
            SET_COOKIE,
            HeaderValue::from_static("__Host-keel_session=ks_abc; Path=/; Secure; HttpOnly"),
        );
        let response = Response {
            url: "https://dns.example/api/v1/call/auth.login".to_owned(),
            status: 200,
            headers,
            body: Vec::new(),
        };
        assert_eq!(
            response.session(),
            Some(Credential::session("ks_abc").unwrap())
        );
        let mut cleared = HeaderMap::new();
        cleared.append(
            SET_COOKIE,
            HeaderValue::from_static("__Host-keel_session=; Path=/; Max-Age=0"),
        );
        let response = Response {
            headers: cleared,
            ..response
        };
        assert_eq!(response.session(), None);
        assert!(!response.replayed());
    }
}
