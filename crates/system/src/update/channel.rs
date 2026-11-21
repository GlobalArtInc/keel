use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json;
use keel_sdk::{KeelError, Result};
use semver::Version;

pub const ERROR_CHANNEL: &str = "update.error.channel";
pub const ERROR_MANIFEST: &str = "update.error.manifest";
pub const ERROR_HEAD_EXPIRED: &str = "update.error.head_expired";
pub const ERROR_HEAD_NO_EXPIRY: &str = "update.error.head_no_expiry";
const MANIFEST_LIMIT: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(30);
const REDIRECT_LIMIT: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub(super) enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
        }
    }

    pub(super) fn head_path(self) -> String {
        format!("{}/manifest.json", self.as_str())
    }
}

impl FromStr for Channel {
    type Err = ();

    fn from_str(text: &str) -> std::result::Result<Self, ()> {
        match text {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            _ => Err(()),
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn manifest_path(version: &Version) -> String {
    format!("versions/{version}/manifest.json")
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(crate = "keel_sdk::serde")]
pub(super) struct Manifest {
    pub(super) version: Version,
    pub(super) sha256: String,
    pub(super) min_upgrade_from: Version,
    pub(super) contract: u32,
    pub(super) notes: String,
    pub(super) expires_at: Option<u64>,
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Envelope {
    payload: String,
}

impl Manifest {
    fn check_fresh(&self, url: &str, now: DateTime<Utc>) -> Result<()> {
        let Some(expires_at) = self.expires_at else {
            return Err(KeelError::unavailable(ERROR_HEAD_NO_EXPIRY).param("url", url));
        };
        let expires = i64::try_from(expires_at).unwrap_or(i64::MAX);
        if expires <= now.timestamp() {
            let shown = DateTime::<Utc>::from_timestamp(expires, 0)
                .map_or_else(|| expires_at.to_string(), |at| at.to_rfc3339());
            return Err(KeelError::unavailable(ERROR_HEAD_EXPIRED)
                .param("url", url)
                .param("expiresAt", shown));
        }
        Ok(())
    }

    pub(super) fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        let envelope: Envelope = serde_json::from_slice(bytes)
            .map_err(|error| format!("not a signed envelope: {error}"))?;
        let payload = STANDARD
            .decode(envelope.payload)
            .map_err(|error| format!("the payload is not base64: {error}"))?;
        serde_json::from_slice(&payload)
            .map_err(|error| format!("the payload is not a release manifest: {error}"))
    }
}

pub(super) struct Source {
    base: String,
    client: reqwest::Client,
}

fn plain_http_allowed(insecure_rehearsal: bool, base: &str) -> bool {
    insecure_rehearsal && base.starts_with("http://")
}

fn redirect_refusal(
    from: &reqwest::Url,
    to: &reqwest::Url,
    allow_plain_http: bool,
) -> Option<&'static str> {
    let scheme_kept = to.scheme() == "https" || (allow_plain_http && to.scheme() == "http");
    if !scheme_kept {
        return Some("the redirect leaves https");
    }
    if from.host_str() != to.host_str()
        || from.port_or_known_default() != to.port_or_known_default()
    {
        return Some("the redirect changes the host");
    }
    None
}

fn redirect_policy(allow_plain_http: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= REDIRECT_LIMIT {
            return attempt.error(format!("more than {REDIRECT_LIMIT} redirects"));
        }
        let Some(from) = attempt.previous().last() else {
            return attempt.error("a redirect without a request before it");
        };
        match redirect_refusal(from, attempt.url(), allow_plain_http) {
            Some(reason) => {
                let message = format!("{reason}: {from} to {}", attempt.url());
                attempt.error(message)
            }
            None => attempt.follow(),
        }
    })
}

impl Source {
    pub(super) fn new(base: &str, insecure_rehearsal: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("keel/{}", crate::VERSION))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(redirect_policy(plain_http_allowed(
                insecure_rehearsal,
                base,
            )))
            .build()
            .map_err(|error| {
                KeelError::internal(ERROR_CHANNEL)
                    .param("url", base)
                    .param("reason", error.to_string())
            })?;
        Ok(Self {
            base: base.to_owned(),
            client,
        })
    }

    pub(super) fn base(&self) -> &str {
        &self.base
    }

    pub(super) async fn head(&self, channel: Channel) -> Result<(Manifest, String)> {
        let url = format!("{}{}", self.base, channel.head_path());
        let bytes = self.fetch(&url).await?.ok_or_else(|| {
            KeelError::unavailable(ERROR_CHANNEL)
                .param("url", url.as_str())
                .param("reason", "404 Not Found")
        })?;
        let head = manifest_of(&url, &bytes)?;
        Ok((head, url))
    }

    pub(super) async fn fresh_head(
        &self,
        channel: Channel,
        now: DateTime<Utc>,
    ) -> Result<Manifest> {
        let (head, url) = self.head(channel).await?;
        head.check_fresh(&url, now)?;
        Ok(head)
    }

    pub(super) async fn manifest(&self, version: &Version) -> Result<Option<Manifest>> {
        let url = format!("{}{}", self.base, manifest_path(version));
        match self.fetch(&url).await? {
            Some(bytes) => Ok(Some(manifest_of(&url, &bytes)?)),
            None => Ok(None),
        }
    }

    async fn fetch(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let failed = |reason: String| {
            KeelError::unavailable(ERROR_CHANNEL)
                .param("url", url)
                .param("reason", reason)
        };
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| failed(describe(&error)))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(failed(response.status().to_string()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| failed(describe(&error)))?
        {
            if bytes.len() + chunk.len() > MANIFEST_LIMIT {
                return Err(failed(format!(
                    "the manifest is over {MANIFEST_LIMIT} bytes"
                )));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    }
}

fn manifest_of(url: &str, bytes: &[u8]) -> Result<Manifest> {
    Manifest::parse(bytes).map_err(|reason| {
        KeelError::unavailable(ERROR_MANIFEST)
            .param("url", url)
            .param("reason", reason)
    })
}

fn describe(error: &reqwest::Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Source").field("base", &self.base).finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    fn envelope(manifest: &str) -> Vec<u8> {
        json!({ "payload": STANDARD.encode(manifest), "signature": STANDARD.encode([7u8; 64]) })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn a_channel_round_trips_through_its_text_and_names_its_head() {
        for channel in [Channel::Stable, Channel::Beta] {
            assert_eq!(channel.as_str().parse::<Channel>(), Ok(channel));
            assert_eq!(json!(channel), json!(channel.as_str()));
            assert_eq!(channel.to_string(), channel.as_str());
        }
        assert_eq!("nightly".parse::<Channel>(), Err(()));
        assert_eq!(Channel::default(), Channel::Stable);
        assert_eq!(Channel::Stable.head_path(), "stable/manifest.json");
        assert_eq!(Channel::Beta.head_path(), "beta/manifest.json");
        assert_eq!(
            manifest_path(&Version::new(1, 3, 0)),
            "versions/1.3.0/manifest.json"
        );
    }

    #[test]
    fn the_payload_of_a_signed_envelope_is_the_manifest_and_anything_else_is_named() {
        let manifest = Manifest::parse(&envelope(
            r#"{"version":"0.2.0","sha256":"ab","min_upgrade_from":"0.1.0","contract":1,"notes":"Second","later":true}"#,
        ))
        .unwrap();
        assert_eq!(manifest.version, Version::new(0, 2, 0));
        assert_eq!(manifest.min_upgrade_from, Version::new(0, 1, 0));
        assert_eq!(manifest.contract, 1);
        assert_eq!(manifest.notes, "Second");
        assert_eq!(manifest.expires_at, None);
        assert!(
            Manifest::parse(b"not json")
                .unwrap_err()
                .starts_with("not a signed envelope")
        );
        assert!(
            Manifest::parse(br#"{"payload":"***","signature":""}"#)
                .unwrap_err()
                .starts_with("the payload is not base64")
        );
        assert!(
            Manifest::parse(&envelope(r#"{"version":1}"#))
                .unwrap_err()
                .starts_with("the payload is not a release manifest")
        );
        assert_eq!(
            manifest_of("http://c/x", b"{}").unwrap_err().key(),
            ERROR_MANIFEST
        );
    }

    #[test]
    fn a_redirect_off_https_or_to_another_host_is_refused_and_a_rehearsal_lifts_plain_http_alone() {
        let url = |text: &str| reqwest::Url::parse(text).unwrap();
        let from = url("https://dl.globalart.hu/keel/stable/manifest.json");
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://dl.globalart.hu/other/manifest.json"),
                false
            ),
            None
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("http://dl.globalart.hu/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect leaves https")
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://mirror.example.org/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect changes the host")
        );
        assert_eq!(
            redirect_refusal(
                &from,
                &url("https://dl.globalart.hu:8443/keel/stable/manifest.json"),
                false
            ),
            Some("the redirect changes the host")
        );
        assert!(!plain_http_allowed(true, "https://dl.globalart.hu/keel/"));
        assert!(!plain_http_allowed(false, "http://127.0.0.1:8080/keel/"));
        assert!(plain_http_allowed(true, "http://127.0.0.1:8080/keel/"));
        assert_eq!(
            redirect_refusal(
                &from,
                &url("http://dl.globalart.hu/keel/stable/manifest.json"),
                plain_http_allowed(true, "https://dl.globalart.hu/keel/")
            ),
            Some("the redirect leaves https"),
            "the rehearsal setting with an https channel does not admit a redirect to http"
        );
        let rehearsal = url("http://127.0.0.1:8080/keel/stable/manifest.json");
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:8080/other"), true),
            None
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:8080/other"), false),
            Some("the redirect leaves https")
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://localhost:8080/other"), true),
            Some("the redirect changes the host")
        );
        assert_eq!(
            redirect_refusal(&rehearsal, &url("http://127.0.0.1:9090/other"), true),
            Some("the redirect changes the host")
        );
    }

    #[test]
    fn a_head_without_an_expiry_or_past_it_is_refused_by_the_kernel_side_read() {
        let now: DateTime<Utc> = "2027-01-15T08:00:00Z".parse().unwrap();
        let head = |expires_at: Option<u64>| Manifest {
            version: Version::new(0, 2, 0),
            sha256: "ab".into(),
            min_upgrade_from: Version::new(0, 1, 0),
            contract: 1,
            notes: String::new(),
            expires_at,
        };
        assert!(
            head(Some(1_800_000_001))
                .check_fresh("http://c/x", now)
                .is_ok()
        );
        let missing = head(None).check_fresh("http://c/x", now).unwrap_err();
        assert_eq!(missing.key(), ERROR_HEAD_NO_EXPIRY);
        assert_eq!(missing.params()["url"], "http://c/x");
        for expires_at in [1_800_000_000, 1] {
            let expired = head(Some(expires_at))
                .check_fresh("http://c/x", now)
                .unwrap_err();
            assert_eq!(expired.key(), ERROR_HEAD_EXPIRED);
        }
        assert_eq!(
            head(Some(1))
                .check_fresh("http://c/x", now)
                .unwrap_err()
                .params()["expiresAt"],
            "1970-01-01T00:00:01+00:00"
        );
        assert!(head(Some(u64::MAX)).check_fresh("http://c/x", now).is_ok());
        let parsed = Manifest::parse(&envelope(
            r#"{"version":"0.2.0","sha256":"ab","min_upgrade_from":"0.1.0","contract":1,"notes":"","expires_at":1800000001}"#,
        ))
        .unwrap();
        assert_eq!(parsed.expires_at, Some(1_800_000_001));
    }
}
