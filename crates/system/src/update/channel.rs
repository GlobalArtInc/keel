use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json;
use keel_sdk::{KeelError, Result};
use semver::Version;

pub const ERROR_CHANNEL: &str = "update.error.channel";
pub const ERROR_MANIFEST: &str = "update.error.manifest";
const MANIFEST_LIMIT: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(30);

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
}

#[derive(Deserialize)]
#[serde(crate = "keel_sdk::serde")]
struct Envelope {
    payload: String,
}

impl Manifest {
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

impl Source {
    pub(super) fn new(base: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("keel/{}", crate::VERSION))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
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

    pub(super) async fn head(&self, channel: Channel) -> Result<Manifest> {
        let url = format!("{}{}", self.base, channel.head_path());
        let bytes = self.fetch(&url).await?.ok_or_else(|| {
            KeelError::unavailable(ERROR_CHANNEL)
                .param("url", url.as_str())
                .param("reason", "404 Not Found")
        })?;
        manifest_of(&url, &bytes)
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
}
