use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::{KeelError, Result};

pub const SECTION: &str = "update";
pub const CHANNEL_URL: &str = "https://dl.globalart.hu/keel/";
pub const ERROR_CHANNEL_URL: &str = "update.error.channel_url";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel_sdk::schemars")]
pub(super) struct UpdateConfig {
    pub(super) channel_url: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel_url: CHANNEL_URL.to_owned(),
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
            channel_url: "http://127.0.0.1:8080/channel".into(),
        };
        assert_eq!(bare.base().unwrap(), "http://127.0.0.1:8080/channel/");
        for wrong in [
            "file:///srv/channel",
            "/srv/channel",
            "ftp://x/",
            "https://x/?a=1",
            "",
        ] {
            let error = UpdateConfig {
                channel_url: wrong.into(),
            }
            .base()
            .unwrap_err();
            assert_eq!(error.key(), ERROR_CHANNEL_URL, "{wrong}");
            assert_eq!(error.params()["url"], wrong, "{wrong}");
        }
    }
}
