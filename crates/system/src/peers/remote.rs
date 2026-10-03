use std::time::Duration;

use keel_sdk::serde::Serialize;
use keel_sdk::serde::de::DeserializeOwned;
use keel_sdk::serde_json::{self, Value};
use keel_sdk::types::{CallError, Envelope, ErrorCode, InstanceInfo};
use keel_sdk::{KeelError, Result};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::redirect::Policy;

pub const INSTANCE_PATH: &str = "/api/v1/instance";
pub const CALL_PATH: &str = "/api/v1/call";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const TIMEOUT: Duration = Duration::from_secs(30);
pub const BODY_CAP: usize = 1 << 20;
pub const ERROR_UNREACHABLE: &str = "peer.error.unreachable";
pub const ERROR_HANDSHAKE: &str = "peer.error.handshake";
pub const ERROR_ANSWER: &str = "peer.error.answer";
pub const ERROR_URL: &str = "peer.error.url";
const JSON: &str = "application/json";
const REASON_CAP: usize = 256;

pub struct Remote {
    base: String,
    client: reqwest::Client,
}

impl Remote {
    pub fn new(url: &str) -> Result<Self> {
        let base = base_of(url);
        let client = reqwest::Client::builder()
            .user_agent(format!("keel/{}", crate::VERSION))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(Policy::none())
            .build()
            .map_err(|error| unreachable(&base, &error.to_string()))?;
        Ok(Self { base, client })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn handshake(&self) -> Result<InstanceInfo> {
        let url = format!("{}{INSTANCE_PATH}", self.base);
        let response = self
            .client
            .get(&url)
            .header(ACCEPT, JSON)
            .send()
            .await
            .map_err(|error| unreachable(&self.base, &describe(&error)))?;
        let status = response.status();
        let body = read(response)
            .await
            .map_err(|reason| unreachable(&self.base, &reason))?;
        if !status.is_success() {
            return Err(KeelError::peer(ERROR_HANDSHAKE).param("url", self.base.as_str()));
        }
        serde_json::from_slice(&body)
            .map_err(|_| KeelError::peer(ERROR_HANDSHAKE).param("url", self.base.as_str()))
    }

    pub async fn call<T: DeserializeOwned>(
        &self,
        func: &str,
        params: &impl Serialize,
    ) -> Result<T> {
        let url = format!("{}{CALL_PATH}/{func}", self.base);
        let response = self
            .client
            .post(&url)
            .header(ACCEPT, JSON)
            .header(CONTENT_TYPE, JSON)
            .body(serde_json::to_vec(params)?)
            .send()
            .await
            .map_err(|error| unreachable(&self.base, &describe(&error)))?;
        let body = read(response)
            .await
            .map_err(|reason| unreachable(&self.base, &reason))?;
        let answer = || KeelError::peer(ERROR_ANSWER).param("url", self.base.as_str());
        match serde_json::from_slice::<Envelope<Value>>(&body).map_err(|_| answer())? {
            Envelope::Ok { data, .. } => serde_json::from_value(data).map_err(|_| answer()),
            Envelope::Err { error } => Err(refused(error)),
        }
    }
}

pub fn base_of(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

pub fn checked(url: &str, field: &str) -> Result<String> {
    let base = base_of(url);
    let plain = reqwest::Url::parse(&base).is_ok_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none()
    });
    if plain {
        Ok(base)
    } else {
        Err(KeelError::validation(ERROR_URL)
            .field(field)
            .param("url", url))
    }
}

pub fn refused(error: CallError) -> KeelError {
    let CallError {
        key,
        params,
        fields,
        ..
    } = error;
    let mut mapped = KeelError::with_key(ErrorCode::Peer, key);
    for (name, value) in params {
        mapped = mapped.param(name, value);
    }
    mapped.fields(fields)
}

fn unreachable(url: &str, reason: &str) -> KeelError {
    KeelError::peer(ERROR_UNREACHABLE)
        .param("url", url)
        .param("reason", truncate(reason))
}

async fn read(mut response: reqwest::Response) -> std::result::Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| describe(&error))? {
        if body.len() + chunk.len() > BODY_CAP {
            return Err(format!("the answer is larger than {BODY_CAP} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn describe(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        let line = inner.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = inner.source();
    }
    text
}

fn truncate(text: &str) -> String {
    if text.len() <= REASON_CAP {
        return text.to_owned();
    }
    let mut end = REASON_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;
    use keel_sdk::types::FieldError;

    use super::*;

    #[test]
    fn a_refusal_of_the_other_side_keeps_its_key_params_and_fields_under_the_peer_code() {
        let mut error = CallError::new(ErrorCode::Validation);
        error.key = "peer.error.code".to_owned();
        error.params.insert("expired".to_owned(), json!(true));
        error.fields.push(FieldError {
            path: "code".to_owned(),
            code: Some("peer.error.code".to_owned()),
            key: Some("peer.error.code".to_owned()),
            params: Default::default(),
        });
        let mapped = refused(error).to_call_error();
        assert_eq!(mapped.code, ErrorCode::Peer);
        assert_eq!(mapped.key, "peer.error.code");
        assert_eq!(mapped.params["expired"], json!(true));
        assert_eq!(mapped.fields.len(), 1);
        assert_eq!(mapped.fields[0].path, "code");
        assert_eq!(mapped.status(), 502);
    }

    #[test]
    fn an_address_is_an_http_or_https_base_without_credentials_query_or_fragment() {
        assert_eq!(
            checked("https://dns.example:8443/", "url").unwrap(),
            "https://dns.example:8443"
        );
        assert_eq!(
            checked("http://10.0.0.5/keel", "url").unwrap(),
            "http://10.0.0.5/keel"
        );
        for refused in [
            "",
            "dns.example",
            "ftp://dns.example",
            "https://user:secret@dns.example",
            "https://dns.example/?a=b",
            "https://dns.example/#top",
            "file:///etc/keel",
        ] {
            let error = checked(refused, "url").unwrap_err().to_call_error();
            assert_eq!(error.fields[0].path, "url", "{refused}");
            assert_eq!(error.fields[0].key.as_deref(), Some(ERROR_URL), "{refused}");
        }
    }

    #[test]
    fn the_base_loses_its_trailing_slashes_and_a_long_reason_is_cut_on_a_character() {
        assert_eq!(
            base_of(" https://dns.example:8443/ "),
            "https://dns.example:8443"
        );
        assert_eq!(base_of("http://127.0.0.1:1//"), "http://127.0.0.1:1");
        let long = "ж".repeat(REASON_CAP);
        let cut = truncate(&long);
        assert!(cut.len() <= REASON_CAP);
        assert!(long.starts_with(&cut));
        assert_eq!(truncate("short"), "short");
    }
}
