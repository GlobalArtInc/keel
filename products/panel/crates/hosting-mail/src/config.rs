use keel::prelude::*;

pub(crate) const SECTION: &str = "hosting_mail";
const MAILDIRS_DIR: &str = "/var/lib/keel-panel-mail";
const DUMPS_DIR: &str = "/var/lib/keel-panel-dumps";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct MailConfig {
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) maildirs_dir: String,
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) dumps_dir: String,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            maildirs_dir: MAILDIRS_DIR.to_owned(),
            dumps_dir: DUMPS_DIR.to_owned(),
        }
    }
}

impl MailConfig {
    pub(crate) fn home(&self, dir: &str) -> String {
        format!("{}/{dir}", self.maildirs_dir.trim_end_matches('/'))
    }

    pub(crate) fn dump_path(&self, file: &str) -> String {
        format!("{}/{file}", self.dumps_dir.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_defaults_to_the_maildir_and_dump_roots_of_the_agent_policy() {
        let config = MailConfig::default();
        assert_eq!(
            config.home("keel-panel-01J9"),
            "/var/lib/keel-panel-mail/keel-panel-01J9"
        );
        assert_eq!(
            config.dump_path("keel-panel-maildirs.tar"),
            "/var/lib/keel-panel-dumps/keel-panel-maildirs.tar"
        );
    }
}
