use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::{KeelError, Result};

pub const SECTION: &str = "update";
pub const CHANNEL_URL: &str = "https://dl.globalart.hu/keel/";
pub const ERROR_CHANNEL_URL: &str = "update.error.channel_url";
pub const ERROR_CHANNEL_INSECURE: &str = "update.error.channel_insecure";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct UpdateConfig {
    pub(super) channel_url: String,
    pub(super) insecure_rehearsal_channel: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel_url: CHANNEL_URL.to_owned(),
            insecure_rehearsal_channel: false,
        }
    }
}

impl UpdateConfig {
    pub(super) fn base(&self) -> Result<String> {
        let url = self.channel_url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(['?', '#'])
        {
            return Err(KeelError::internal(ERROR_CHANNEL_URL).param("url", url));
        }
        if url.starts_with("http://") && !self.insecure_rehearsal_channel {
            return Err(KeelError::internal(ERROR_CHANNEL_INSECURE).param("url", url));
        }
        Ok(format!("{}/", url.trim_end_matches('/')))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_channel_is_the_release_channel_and_a_base_ends_with_one_slash() {
        let config = UpdateConfig::default();
        assert_eq!(config.channel_url, "https://dl.globalart.hu/keel/");
        assert_eq!(config.base().unwrap(), "https://dl.globalart.hu/keel/");
        let bare = UpdateConfig {
            channel_url: "https://dl.example.org/channel".into(),
            ..UpdateConfig::default()
        };
        assert_eq!(bare.base().unwrap(), "https://dl.example.org/channel/");
        for wrong in [
            "file:///srv/channel",
            "/srv/channel",
            "ftp://x/",
            "https://x/?a=1",
            "",
        ] {
            let error = UpdateConfig {
                channel_url: wrong.into(),
                ..UpdateConfig::default()
            }
            .base()
            .unwrap_err();
            assert_eq!(error.key(), ERROR_CHANNEL_URL, "{wrong}");
            assert_eq!(error.params()["url"], wrong, "{wrong}");
        }
    }

    #[test]
    fn a_plain_http_channel_is_refused_without_the_rehearsal_flag() {
        let plain = UpdateConfig {
            channel_url: "http://127.0.0.1:8080/channel".into(),
            ..UpdateConfig::default()
        };
        assert!(!plain.insecure_rehearsal_channel);
        let error = plain.base().unwrap_err();
        assert_eq!(error.key(), ERROR_CHANNEL_INSECURE);
        assert_eq!(error.params()["url"], "http://127.0.0.1:8080/channel");
        let rehearsal = UpdateConfig {
            insecure_rehearsal_channel: true,
            ..plain
        };
        assert_eq!(rehearsal.base().unwrap(), "http://127.0.0.1:8080/channel/");
        let https = UpdateConfig {
            insecure_rehearsal_channel: true,
            ..UpdateConfig::default()
        };
        assert_eq!(https.base().unwrap(), "https://dl.globalart.hu/keel/");
        let malformed = UpdateConfig {
            channel_url: "ftp://x/".into(),
            insecure_rehearsal_channel: true,
        };
        assert_eq!(malformed.base().unwrap_err().key(), ERROR_CHANNEL_URL);
    }
}
