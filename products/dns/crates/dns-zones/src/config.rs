use keel::prelude::*;

pub(crate) const SECTION: &str = "dns";

const ZONES_DIR: &str = "/etc/bind/zones";
const INCLUDE_DIR: &str = "/etc/bind/keel";
const NAMESERVER: &str = "localhost";
const HOSTMASTER: &str = "hostmaster.localhost";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde", default, deny_unknown_fields)]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct DnsConfig {
    #[schemars(length(min = 2, max = 200), pattern(r#"^/[^\s";]*$"#))]
    pub(crate) zones_dir: String,
    #[schemars(length(min = 2, max = 200), pattern(r#"^/[^\s";]*$"#))]
    pub(crate) include_dir: String,
    #[schemars(
        length(min = 1, max = 253),
        pattern(r"^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$")
    )]
    pub(crate) nameserver: String,
    #[schemars(
        length(min = 1, max = 253),
        pattern(r"^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$")
    )]
    pub(crate) hostmaster: String,
}

impl Default for DnsConfig {
    fn default() -> Self {
        Self {
            zones_dir: ZONES_DIR.to_owned(),
            include_dir: INCLUDE_DIR.to_owned(),
            nameserver: NAMESERVER.to_owned(),
            hostmaster: HOSTMASTER.to_owned(),
        }
    }
}

impl DnsConfig {
    pub(crate) fn zone_path(&self, file: &str) -> String {
        format!("{}/{file}", self.zones_dir.trim_end_matches('/'))
    }

    pub(crate) fn include_path(&self, file: &str) -> String {
        format!("{}/{file}", self.include_dir.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_defaults_to_the_debian_layout_of_bind_and_joins_file_names() {
        let config = DnsConfig::default();
        assert_eq!(
            config.zone_path("keel-dns-a.org.zone"),
            "/etc/bind/zones/keel-dns-a.org.zone"
        );
        assert_eq!(
            config.include_path("keel-dns-a.org.conf"),
            "/etc/bind/keel/keel-dns-a.org.conf"
        );
        let slashed = DnsConfig {
            zones_dir: "/srv/zones/".to_owned(),
            ..DnsConfig::default()
        };
        assert_eq!(slashed.zone_path("z"), "/srv/zones/z");
        let parsed: DnsConfig =
            keel::sdk::serde_json::from_value(json!({ "nameserver": "ns1.example.net" }))
                .expect("a partial section parses over the defaults");
        assert_eq!(parsed.nameserver, "ns1.example.net");
        assert_eq!(parsed.zones_dir, ZONES_DIR);
    }
}
