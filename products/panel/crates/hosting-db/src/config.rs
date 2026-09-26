use keel::prelude::*;

pub(crate) const SECTION: &str = "hosting_database";
const DUMPS_DIR: &str = "/var/lib/keel-panel-dumps";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct DatabaseConfig {
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) dumps_dir: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            dumps_dir: DUMPS_DIR.to_owned(),
        }
    }
}

impl DatabaseConfig {
    pub(crate) fn dump_path(&self, file: &str) -> String {
        format!("{}/{file}", self.dumps_dir.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_defaults_to_the_dump_root_of_the_agent_policy() {
        assert_eq!(
            DatabaseConfig::default().dump_path("keel-panel-01J9.sql"),
            "/var/lib/keel-panel-dumps/keel-panel-01J9.sql"
        );
    }
}
