use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
    #[error("the check {check} has no command")]
    EmptyCheck { check: String },
    #[error("the product {product} names an empty {prefix}")]
    EmptyPrefix { product: String, prefix: String },
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
    pub file_roots: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub checks: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub products: BTreeMap<String, Product>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Product {
    #[serde(default)]
    pub user_prefix: Option<String>,
    #[serde(default)]
    pub drop_in_prefix: Option<String>,
}

impl Policy {
    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        let text = fs::read_to_string(path).map_err(|source| PolicyError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, PolicyError> {
        let file: File = serde_yaml_ng::from_str(text).map_err(PolicyError::Parse)?;
        let policy = file.policy;
        for (root, path) in &policy.file_roots {
            if !path.is_absolute() {
                return Err(PolicyError::RelativeRoot {
                    root: root.clone(),
                    path: path.clone(),
                });
            }
        }
        for (check, argv) in &policy.checks {
            if argv.is_empty() {
                return Err(PolicyError::EmptyCheck {
                    check: check.clone(),
                });
            }
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
        }
        Ok(policy)
    }

    pub fn product(&self, id: &str) -> Option<&Product> {
        self.products.get(id)
    }

    pub fn root(&self, name: &str) -> Option<&Path> {
        self.file_roots.get(name).map(PathBuf::as_path)
    }

    pub fn check(&self, name: &str) -> Option<&[String]> {
        self.checks.get(name).map(Vec::as_slice)
    }

    pub fn allows_service(&self, name: &str) -> bool {
        self.services.iter().any(|service| service == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: &str = r#"
policy:
  file_roots: { zones: /etc/bind/zones, vhosts: /etc/nginx/conf.d }
  services: [nginx, named]
  checks: { nginx.test: ["nginx", "-t"] }
  products:
    demo: { user_prefix: kd_, drop_in_prefix: keel-demo- }
    dns:  { drop_in_prefix: keel-dns- }
"#;

    #[test]
    fn the_policy_in_the_shape_of_the_design_parses_into_its_sections() {
        let policy = Policy::parse(SHAPE).unwrap();
        assert_eq!(policy.root("vhosts"), Some(Path::new("/etc/nginx/conf.d")));
        assert_eq!(policy.root("etc"), None);
        assert!(policy.allows_service("nginx"));
        assert!(!policy.allows_service("sshd"));
        assert_eq!(
            policy.check("nginx.test"),
            Some(["nginx".to_owned(), "-t".to_owned()].as_slice())
        );
        assert_eq!(policy.check("other"), None);
        assert_eq!(
            policy.product("demo"),
            Some(&Product {
                user_prefix: Some("kd_".to_owned()),
                drop_in_prefix: Some("keel-demo-".to_owned()),
            })
        );
        assert_eq!(policy.product("dns").unwrap().user_prefix, None);
        assert_eq!(policy.product("panel"), None);
    }

    #[test]
    fn the_shipped_policy_of_the_spike_parses() {
        let policy = Policy::parse(include_str!("../deploy/spike.yaml")).unwrap();
        assert_eq!(policy.root("vhosts"), Some(Path::new("/etc/nginx/conf.d")));
        assert_eq!(policy.services, vec!["nginx".to_owned()]);
        assert_eq!(policy.check("nginx.test").unwrap(), ["nginx", "-t"]);
        assert_eq!(
            policy.product("demo").unwrap().drop_in_prefix.as_deref(),
            Some("keel-demo-")
        );
        assert_eq!(
            policy.product("demo").unwrap().user_prefix.as_deref(),
            Some("kd_")
        );
    }

    #[test]
    fn a_relative_root_an_empty_check_an_empty_prefix_and_an_unknown_key_are_refused() {
        let relative = "policy:\n  file_roots: { vhosts: etc/nginx }\n";
        assert!(matches!(
            Policy::parse(relative).unwrap_err(),
            PolicyError::RelativeRoot { root, .. } if root == "vhosts"
        ));
        let empty_check = "policy:\n  checks: { nginx.test: [] }\n";
        assert!(matches!(
            Policy::parse(empty_check).unwrap_err(),
            PolicyError::EmptyCheck { check } if check == "nginx.test"
        ));
        let empty_prefix = "policy:\n  products: { demo: { user_prefix: \"\" } }\n";
        assert!(matches!(
            Policy::parse(empty_prefix).unwrap_err(),
            PolicyError::EmptyPrefix { product, prefix } if product == "demo" && prefix == "user_prefix"
        ));
        let unknown = "policy:\n  exec_allow: [certbot]\n";
        assert!(matches!(
            Policy::parse(unknown).unwrap_err(),
            PolicyError::Parse(_)
        ));
        let no_policy = "file_roots: {}\n";
        assert!(matches!(
            Policy::parse(no_policy).unwrap_err(),
            PolicyError::Parse(_)
        ));
    }
}
