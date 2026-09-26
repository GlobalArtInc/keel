use keel::prelude::*;

pub(crate) const SECTION: &str = "hosting_certificate";

const CERTS_DIR: &str = "/etc/ssl/keel";
const CHALLENGES_DIR: &str = "/var/lib/keel-panel-acme";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct CertificateConfig {
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) certs_dir: String,
    #[schemars(length(min = 2, max = 200), pattern(r"^(/[A-Za-z0-9._-]+)+$"))]
    pub(crate) challenges_dir: String,
    #[schemars(length(max = 500), pattern(r"^(|/[^\s]*)$"))]
    pub(crate) directory_ca: String,
}

impl Default for CertificateConfig {
    fn default() -> Self {
        Self {
            certs_dir: CERTS_DIR.to_owned(),
            challenges_dir: CHALLENGES_DIR.to_owned(),
            directory_ca: String::new(),
        }
    }
}

impl CertificateConfig {
    pub(crate) fn cert_path(&self, file: &str) -> String {
        format!("{}/{file}", self.certs_dir.trim_end_matches('/'))
    }

    pub(crate) fn challenges(&self) -> String {
        self.challenges_dir.trim_end_matches('/').to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_defaults_to_the_roots_of_the_agent_policy_and_joins_file_names() {
        let config = CertificateConfig::default();
        assert_eq!(
            config.cert_path("keel-panel-01J9.crt"),
            "/etc/ssl/keel/keel-panel-01J9.crt"
        );
        assert_eq!(config.challenges(), "/var/lib/keel-panel-acme");
        assert!(config.directory_ca.is_empty());
    }
}
