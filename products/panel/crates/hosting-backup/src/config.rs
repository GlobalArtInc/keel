use keel::prelude::*;

pub(crate) const SECTION: &str = "hosting_backup";
const ARCHIVES_DIR: &str = "/var/lib/keel-panel-backups";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct BackupConfig {
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) archives_dir: String,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            archives_dir: ARCHIVES_DIR.to_owned(),
        }
    }
}

impl BackupConfig {
    pub(crate) fn archive_path(&self, file: &str) -> String {
        format!("{}/{file}", self.archives_dir.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_defaults_to_the_backups_root_of_the_agent_policy() {
        assert_eq!(
            BackupConfig::default().archive_path("keel-panel-01J9.tar.zst"),
            "/var/lib/keel-panel-backups/keel-panel-01J9.tar.zst"
        );
    }
}
