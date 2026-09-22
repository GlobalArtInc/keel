use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_SYSTEMCTL: &str = "systemctl";
const DEFAULT_GETENT: &str = "getent";
const DEFAULT_USERADD: &str = "useradd";
const DEFAULT_USERDEL: &str = "userdel";
const DEFAULT_GROUPADD: &str = "groupadd";
const DEFAULT_GROUPDEL: &str = "groupdel";
const DEFAULT_SHELLS: &str = "/etc/shells";

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("could not read the policy {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the policy is not valid: {0}")]
    Parse(#[source] serde_yaml_ng::Error),
    #[error("the file root {root} is not an absolute path: {path}")]
    RelativeRoot { root: String, path: PathBuf },
    #[error("the service {0} is not a unit name")]
    ServiceName(String),
    #[error("the check {check} has no command")]
    EmptyCheck { check: String },
    #[error("exec_allow carries an empty entry")]
    EmptyExecAllow,
    #[error("the product {product} names an empty {prefix}")]
    EmptyPrefix { product: String, prefix: String },
    #[error("the user_prefix {prefix} of the product {product} is not a prefix of an account name")]
    UserPrefix { product: String, prefix: String },
    #[error("uid_range {from}..{to} is not a range above 0")]
    UidRange { from: u32, to: u32 },
    #[error("the tool {tool} has an empty path")]
    EmptyTool { tool: &'static str },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    policy: Policy,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    uid_range: Option<UidRange>,
    #[serde(default)]
    file_roots: BTreeMap<String, PathBuf>,
    #[serde(default)]
    services: Vec<String>,
    #[serde(default)]
    checks: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    exec_allow: Vec<String>,
    #[serde(default)]
    products: BTreeMap<String, Product>,
    #[serde(default)]
    tools: Tools,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UidRange {
    pub(crate) from: u32,
    pub(crate) to: u32,
}

impl UidRange {
    pub(crate) fn contains(&self, id: u32) -> bool {
        (self.from..=self.to).contains(&id)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Product {
    #[serde(default)]
    pub(crate) user_prefix: Option<String>,
    #[serde(default)]
    pub(crate) drop_in_prefix: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tools {
    #[serde(default = "default_systemctl")]
    pub(crate) systemctl: PathBuf,
    #[serde(default = "default_getent")]
    pub(crate) getent: PathBuf,
    #[serde(default = "default_useradd")]
    pub(crate) useradd: PathBuf,
    #[serde(default = "default_userdel")]
    pub(crate) userdel: PathBuf,
    #[serde(default = "default_groupadd")]
    pub(crate) groupadd: PathBuf,
    #[serde(default = "default_groupdel")]
    pub(crate) groupdel: PathBuf,
    #[serde(default = "default_shells")]
    pub(crate) shells: PathBuf,
}

fn default_systemctl() -> PathBuf {
    PathBuf::from(DEFAULT_SYSTEMCTL)
}

fn default_getent() -> PathBuf {
    PathBuf::from(DEFAULT_GETENT)
}

fn default_useradd() -> PathBuf {
    PathBuf::from(DEFAULT_USERADD)
}

fn default_userdel() -> PathBuf {
    PathBuf::from(DEFAULT_USERDEL)
}

fn default_groupadd() -> PathBuf {
    PathBuf::from(DEFAULT_GROUPADD)
}

fn default_groupdel() -> PathBuf {
    PathBuf::from(DEFAULT_GROUPDEL)
}

fn default_shells() -> PathBuf {
    PathBuf::from(DEFAULT_SHELLS)
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            systemctl: default_systemctl(),
            getent: default_getent(),
            useradd: default_useradd(),
            userdel: default_userdel(),
            groupadd: default_groupadd(),
            groupdel: default_groupdel(),
            shells: default_shells(),
        }
    }
}

impl Policy {
    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        let text = fs::read_to_string(path).map_err(|source| PolicyError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, PolicyError> {
        let file: File = serde_yaml_ng::from_str(text).map_err(PolicyError::Parse)?;
        let policy = file.policy;
        if let Some(range) = policy.uid_range
            && (range.from == 0 || range.from > range.to)
        {
            return Err(PolicyError::UidRange {
                from: range.from,
                to: range.to,
            });
        }
        for (root, path) in &policy.file_roots {
            if !path.is_absolute() {
                return Err(PolicyError::RelativeRoot {
                    root: root.clone(),
                    path: path.clone(),
                });
            }
        }
        for service in &policy.services {
            if !valid_unit_name(service) {
                return Err(PolicyError::ServiceName(service.clone()));
            }
        }
        for (check, argv) in &policy.checks {
            if argv.first().is_none_or(String::is_empty) {
                return Err(PolicyError::EmptyCheck {
                    check: check.clone(),
                });
            }
        }
        if policy.exec_allow.iter().any(String::is_empty) {
            return Err(PolicyError::EmptyExecAllow);
        }
        for (product, section) in &policy.products {
            for (prefix, value) in [
                ("user_prefix", &section.user_prefix),
                ("drop_in_prefix", &section.drop_in_prefix),
            ] {
                if value.as_deref() == Some("") {
                    return Err(PolicyError::EmptyPrefix {
                        product: product.clone(),
                        prefix: prefix.to_owned(),
                    });
                }
            }
            if let Some(prefix) = &section.user_prefix
                && !valid_user_prefix(prefix)
            {
                return Err(PolicyError::UserPrefix {
                    product: product.clone(),
                    prefix: prefix.clone(),
                });
            }
        }
        for (tool, path) in [
            ("systemctl", &policy.tools.systemctl),
            ("getent", &policy.tools.getent),
            ("useradd", &policy.tools.useradd),
            ("userdel", &policy.tools.userdel),
            ("groupadd", &policy.tools.groupadd),
            ("groupdel", &policy.tools.groupdel),
            ("shells", &policy.tools.shells),
        ] {
            if path.as_os_str().is_empty() {
                return Err(PolicyError::EmptyTool { tool });
            }
        }
        Ok(policy)
    }

    pub(crate) fn uid_range(&self) -> Option<UidRange> {
        self.uid_range
    }

    pub(crate) fn product(&self, id: &str) -> Option<&Product> {
        self.products.get(id)
    }

    pub(crate) fn root(&self, name: &str) -> Option<&Path> {
        self.file_roots.get(name).map(PathBuf::as_path)
    }

    pub(crate) fn check(&self, name: &str) -> Option<&[String]> {
        self.checks.get(name).map(Vec::as_slice)
    }

    pub(crate) fn services(&self) -> &[String] {
        &self.services
    }

    pub(crate) fn allows_service(&self, name: &str) -> bool {
        self.services.iter().any(|service| service == name)
    }

    pub(crate) fn tools(&self) -> &Tools {
        &self.tools
    }
}

fn valid_unit_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':' | '\\'))
}

fn valid_user_prefix(prefix: &str) -> bool {
    let mut chars = prefix.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: &str = r#"
policy:
  file_roots: { zones: /etc/bind/zones, vhosts: /etc/nginx/conf.d, www: /var/www }
  services: [nginx, php8.3-fpm, named]
  checks: { nginx.test: ["nginx", "-t"] }
  exec_allow: [certbot, quotatool]
  products:
    panel: { user_prefix: hp_, drop_in_prefix: keel-panel- }
    dns:   { drop_in_prefix: keel-dns- }
"#;

    #[test]
    fn the_policy_in_the_shape_of_the_design_parses_into_its_sections() {
        let policy = Policy::parse(SHAPE).unwrap();
        assert_eq!(policy.root("vhosts"), Some(Path::new("/etc/nginx/conf.d")));
        assert_eq!(policy.root("etc"), None);
        assert!(policy.allows_service("nginx"));
        assert!(policy.allows_service("php8.3-fpm"));
        assert!(!policy.allows_service("sshd"));
        assert_eq!(policy.services(), ["nginx", "php8.3-fpm", "named"]);
        assert_eq!(
            policy.check("nginx.test"),
            Some(["nginx".to_owned(), "-t".to_owned()].as_slice())
        );
        assert_eq!(policy.check("other"), None);
        assert_eq!(
            policy.product("panel"),
            Some(&Product {
                user_prefix: Some("hp_".to_owned()),
                drop_in_prefix: Some("keel-panel-".to_owned()),
            })
        );
        assert_eq!(policy.product("dns").unwrap().user_prefix, None);
        assert_eq!(policy.product("bill"), None);
        assert_eq!(policy.uid_range(), None);
        assert_eq!(policy.tools(), &Tools::default());
        assert_eq!(policy.tools().systemctl, Path::new("systemctl"));
        assert_eq!(policy.tools().getent, Path::new("getent"));
    }

    #[test]
    fn the_shipped_policy_parses_with_its_uid_range_and_a_section_per_product() {
        let policy = Policy::parse(include_str!("../../../deploy/agent.yaml")).unwrap();
        let range = policy.uid_range().unwrap();
        assert_eq!(range.from, 100_000);
        assert_eq!(range.to, 165_535);
        assert!(range.contains(100_000));
        assert!(range.contains(165_535));
        assert!(!range.contains(99_999));
        assert!(!range.contains(165_536));
        assert_eq!(policy.root("zones"), Some(Path::new("/etc/bind/zones")));
        assert!(policy.allows_service("named"));
        for product in ["bill", "dns", "panel"] {
            assert_eq!(
                policy.product(product).unwrap().drop_in_prefix.as_deref(),
                Some(format!("keel-{product}-").as_str())
            );
        }
        assert_eq!(
            policy.product("panel").unwrap().user_prefix.as_deref(),
            Some("hp_")
        );
    }

    #[test]
    fn the_tools_section_points_the_binaries_elsewhere() {
        let policy = Policy::parse(
            "policy:\n  tools: { systemctl: /tmp/stub/systemctl, getent: /tmp/stub/getent }\n",
        )
        .unwrap();
        assert_eq!(policy.tools().systemctl, Path::new("/tmp/stub/systemctl"));
        assert_eq!(policy.tools().getent, Path::new("/tmp/stub/getent"));
        assert_eq!(policy.tools().useradd, Path::new("useradd"));
        assert_eq!(policy.tools().shells, Path::new("/etc/shells"));
        assert!(matches!(
            Policy::parse("policy:\n  tools: { useradd: \"\" }\n").unwrap_err(),
            PolicyError::EmptyTool { tool: "useradd" }
        ));
        assert!(matches!(
            Policy::parse("policy:\n  tools: { sudo: /usr/bin/sudo }\n").unwrap_err(),
            PolicyError::Parse(_)
        ));
    }

    #[test]
    fn a_broken_range_root_service_check_prefix_or_key_is_refused() {
        assert!(matches!(
            Policy::parse("policy:\n  uid_range: { from: 0, to: 10 }\n").unwrap_err(),
            PolicyError::UidRange { from: 0, to: 10 }
        ));
        assert!(matches!(
            Policy::parse("policy:\n  uid_range: { from: 20, to: 10 }\n").unwrap_err(),
            PolicyError::UidRange { from: 20, to: 10 }
        ));
        assert!(matches!(
            Policy::parse("policy:\n  uid_range: { from: 10 }\n").unwrap_err(),
            PolicyError::Parse(_)
        ));
        assert!(matches!(
            Policy::parse("policy:\n  file_roots: { vhosts: etc/nginx }\n").unwrap_err(),
            PolicyError::RelativeRoot { root, .. } if root == "vhosts"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  services: [\"nginx; rm -rf /\"]\n").unwrap_err(),
            PolicyError::ServiceName(name) if name == "nginx; rm -rf /"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  checks: { nginx.test: [] }\n").unwrap_err(),
            PolicyError::EmptyCheck { check } if check == "nginx.test"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  checks: { nginx.test: [\"\"] }\n").unwrap_err(),
            PolicyError::EmptyCheck { .. }
        ));
        assert!(matches!(
            Policy::parse("policy:\n  exec_allow: [certbot, \"\"]\n").unwrap_err(),
            PolicyError::EmptyExecAllow
        ));
        assert!(matches!(
            Policy::parse("policy:\n  products: { demo: { user_prefix: \"\" } }\n").unwrap_err(),
            PolicyError::EmptyPrefix { product, prefix } if product == "demo" && prefix == "user_prefix"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  products: { demo: { user_prefix: \"Hp-\" } }\n").unwrap_err(),
            PolicyError::UserPrefix { product, prefix } if product == "demo" && prefix == "Hp-"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  sudoers: [alice]\n").unwrap_err(),
            PolicyError::Parse(_)
        ));
        assert!(matches!(
            Policy::parse("file_roots: {}\n").unwrap_err(),
            PolicyError::Parse(_)
        ));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Policy::load(&dir.path().join("missing.yaml")).unwrap_err(),
            PolicyError::Read { .. }
        ));
    }
}
