use std::path::PathBuf;

use keel::prelude::*;

pub(crate) const SECTION: &str = "hosting";
const TRANSFER_DIR: &str = "/var/lib/keel-panel-transfer";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct HostingConfig {
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) transfer_dir: String,
}

impl Default for HostingConfig {
    fn default() -> Self {
        Self {
            transfer_dir: TRANSFER_DIR.to_owned(),
        }
    }
}

impl HostingConfig {
    pub(crate) fn transfer_path(&self, name: &str) -> PathBuf {
        PathBuf::from(&self.transfer_dir).join(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transfer_directory_defaults_to_the_root_the_policy_ships() {
        let config = HostingConfig::default();
        assert_eq!(
            config.transfer_path("keel-panel-upload-01J9"),
            PathBuf::from("/var/lib/keel-panel-transfer/keel-panel-upload-01J9")
        );
    }
}
