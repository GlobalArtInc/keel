use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};
use serde_json::Value;

const DEFAULT_SYSTEMCTL: &str = "systemctl";
const DEFAULT_GETENT: &str = "getent";
const DEFAULT_USERADD: &str = "useradd";
const DEFAULT_USERDEL: &str = "userdel";
const DEFAULT_GROUPADD: &str = "groupadd";
const DEFAULT_GROUPDEL: &str = "groupdel";
const DEFAULT_USERMOD: &str = "usermod";
const DEFAULT_SHELLS: &str = "/etc/shells";
const DEFAULT_APT_GET: &str = "apt-get";
const DEFAULT_DPKG_QUERY: &str = "dpkg-query";
const DEFAULT_APT_CACHE: &str = "apt-cache";
const DEFAULT_SETQUOTA: &str = "setquota";
const DEFAULT_REPQUOTA: &str = "repquota";
const DEFAULT_CRON_DIR: &str = "/etc/cron.d";
const VERSION_WILDCARD: char = '*';

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
    #[error(
        "the account root {root} of the product {product} is not a file root, or the product has no user_prefix to name its files"
    )]
    AccountRoot { product: String, root: String },
    #[error("uid_range {from}..{to} is not a range above 0 that stays below {max}", max = u32::MAX)]
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
    #[serde(default, deserialize_with = "exec_entries")]
    exec_allow: BTreeMap<String, Vec<String>>,
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
    #[serde(default)]
    pub(crate) account_roots: Vec<String>,
    #[serde(default)]
    pub(crate) classes: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ExecEntry {
    Binary(String),
    Commands(BTreeMap<String, Vec<String>>),
}

fn exec_entries<'de, D>(deserializer: D) -> Result<BTreeMap<String, Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let mut allowed = BTreeMap::new();
    for entry in Vec::<ExecEntry>::deserialize(deserializer)? {
        let commands = match entry {
            ExecEntry::Binary(binary) => vec![(binary.clone(), vec![binary])],
            ExecEntry::Commands(commands) => commands.into_iter().collect(),
        };
        for (name, argv) in commands {
            if allowed.insert(name.clone(), argv).is_some() {
                return Err(serde::de::Error::custom(format!(
                    "exec_allow names {name} twice"
                )));
            }
        }
    }
    Ok(allowed)
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
    #[serde(default = "default_usermod")]
    pub(crate) usermod: PathBuf,
    #[serde(default = "default_shells")]
    pub(crate) shells: PathBuf,
    #[serde(default = "default_apt_get")]
    pub(crate) apt_get: PathBuf,
    #[serde(default = "default_dpkg_query")]
    pub(crate) dpkg_query: PathBuf,
    #[serde(default = "default_apt_cache")]
    pub(crate) apt_cache: PathBuf,
    #[serde(default = "default_setquota")]
    pub(crate) setquota: PathBuf,
    #[serde(default = "default_repquota")]
    pub(crate) repquota: PathBuf,
    #[serde(default = "default_cron_dir")]
    pub(crate) cron_dir: PathBuf,
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

fn default_usermod() -> PathBuf {
    PathBuf::from(DEFAULT_USERMOD)
}

fn default_shells() -> PathBuf {
    PathBuf::from(DEFAULT_SHELLS)
}

fn default_apt_get() -> PathBuf {
    PathBuf::from(DEFAULT_APT_GET)
}

fn default_dpkg_query() -> PathBuf {
    PathBuf::from(DEFAULT_DPKG_QUERY)
}

fn default_apt_cache() -> PathBuf {
    PathBuf::from(DEFAULT_APT_CACHE)
}

fn default_setquota() -> PathBuf {
    PathBuf::from(DEFAULT_SETQUOTA)
}

fn default_repquota() -> PathBuf {
    PathBuf::from(DEFAULT_REPQUOTA)
}

fn default_cron_dir() -> PathBuf {
    PathBuf::from(DEFAULT_CRON_DIR)
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
            usermod: default_usermod(),
            shells: default_shells(),
            apt_get: default_apt_get(),
            dpkg_query: default_dpkg_query(),
            apt_cache: default_apt_cache(),
            setquota: default_setquota(),
            repquota: default_repquota(),
            cron_dir: default_cron_dir(),
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
            && (range.from == 0 || range.from > range.to || range.to == u32::MAX)
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
            if !valid_service_entry(service) {
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
        if policy
            .exec_allow
            .iter()
            .any(|(name, argv)| name.is_empty() || argv.first().is_none_or(String::is_empty))
        {
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
            if let Some(root) = section.account_roots.iter().find(|root| {
                section.user_prefix.is_none() || !policy.file_roots.contains_key(root.as_str())
            }) {
                return Err(PolicyError::AccountRoot {
                    product: product.clone(),
                    root: root.clone(),
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
            ("usermod", &policy.tools.usermod),
            ("shells", &policy.tools.shells),
            ("apt_get", &policy.tools.apt_get),
            ("dpkg_query", &policy.tools.dpkg_query),
            ("apt_cache", &policy.tools.apt_cache),
            ("setquota", &policy.tools.setquota),
            ("repquota", &policy.tools.repquota),
            ("cron_dir", &policy.tools.cron_dir),
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

    pub(crate) fn exec(&self, name: &str) -> Option<&[String]> {
        self.exec_allow.get(name).map(Vec::as_slice)
    }

    pub(crate) fn services(&self) -> &[String] {
        &self.services
    }

    pub(crate) fn allows_service(&self, name: &str) -> bool {
        self.services
            .iter()
            .any(|service| service_matches(service, name))
    }

    pub(crate) fn tools(&self) -> &Tools {
        &self.tools
    }
}

fn valid_unit_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.len() <= 255
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ':' | '\\'))
}

fn valid_service_entry(entry: &str) -> bool {
    match entry.split_once(VERSION_WILDCARD) {
        None => valid_unit_name(entry),
        Some((head, tail)) => {
            !head.is_empty()
                && !tail.contains(VERSION_WILDCARD)
                && valid_unit_name(&format!("{head}0{tail}"))
        }
    }
}

pub(crate) fn is_pattern(entry: &str) -> bool {
    entry.contains(VERSION_WILDCARD)
}

pub(crate) fn service_matches(entry: &str, name: &str) -> bool {
    match entry.split_once(VERSION_WILDCARD) {
        None => entry == name,
        Some((head, tail)) => name
            .strip_prefix(head)
            .and_then(|rest| rest.strip_suffix(tail))
            .is_some_and(|version| {
                !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.')
            }),
    }
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
                account_roots: Vec::new(),
                classes: BTreeMap::new(),
            })
        );
        assert_eq!(policy.product("dns").unwrap().user_prefix, None);
        assert_eq!(
            policy.exec("certbot"),
            Some(["certbot".to_owned()].as_slice())
        );
        assert_eq!(policy.exec("sh"), None);
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
        assert_eq!(policy.root("named"), Some(Path::new("/etc/bind/keel")));
        for root in ["www", "vhosts", "mail", "certs"] {
            assert!(policy.root(root).is_some_and(Path::is_absolute), "{root}");
        }
        for version in ["8.2", "8.3", "8.4"] {
            assert_eq!(
                policy.root(&format!("pools-{version}")),
                Some(Path::new(&format!("/etc/php/{version}/fpm/pool.d")))
            );
            assert_eq!(
                policy.check(&format!("php{version}-fpm.test")),
                Some([format!("php-fpm{version}"), "-t".to_owned()].as_slice())
            );
        }
        assert_eq!(
            policy.check("nginx.test"),
            Some(["nginx".to_owned(), "-t".to_owned()].as_slice())
        );
        for service in [
            "named",
            "nginx",
            "php8.4-fpm",
            "postfix",
            "dovecot",
            "mariadb",
            "vsftpd",
        ] {
            assert!(policy.allows_service(service), "{service}");
        }
        assert_eq!(policy.tools().usermod, Path::new("usermod"));
        assert_eq!(
            policy.root("dumps"),
            Some(Path::new("/var/lib/keel-panel-dumps"))
        );
        assert_eq!(
            policy
                .exec("mariadb")
                .and_then(<[String]>::first)
                .map(String::as_str),
            Some("mariadb")
        );
        assert_eq!(
            policy
                .exec("mariadb-dump")
                .and_then(<[String]>::first)
                .map(String::as_str),
            Some("mariadb-dump")
        );
        assert_eq!(policy.root("ftp"), Some(Path::new("/etc/vsftpd/keel")));
        assert_eq!(
            policy.root("ftp-users"),
            Some(Path::new("/etc/vsftpd/keel-users"))
        );
        assert_eq!(
            policy.product("panel").unwrap().account_roots,
            ["ftp-users"]
        );
        assert_eq!(
            policy.check("vsftpd.check"),
            Some(
                [
                    "vsftpd".to_owned(),
                    "/etc/vsftpd.conf".to_owned(),
                    "-v".to_owned()
                ]
                .as_slice()
            )
        );
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
        assert_eq!(policy.tools().apt_get, Path::new("apt-get"));
        assert_eq!(policy.tools().dpkg_query, Path::new("dpkg-query"));
        assert_eq!(policy.tools().apt_cache, Path::new("apt-cache"));
        assert_eq!(policy.tools().setquota, Path::new("setquota"));
        assert_eq!(policy.tools().repquota, Path::new("repquota"));
        assert_eq!(policy.tools().cron_dir, Path::new("/etc/cron.d"));
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
    fn exec_allow_names_a_binary_or_a_fixed_argument_vector_once() {
        let policy = Policy::parse(
            "policy:\n  exec_allow:\n    - certbot\n    - { certbot.renew: [certbot, renew, --quiet], keel.version: [/opt/keel/bin/current, --version] }\n",
        )
        .unwrap();
        assert_eq!(
            policy.exec("certbot"),
            Some(["certbot".to_owned()].as_slice())
        );
        assert_eq!(
            policy.exec("certbot.renew"),
            Some(
                [
                    "certbot".to_owned(),
                    "renew".to_owned(),
                    "--quiet".to_owned()
                ]
                .as_slice()
            )
        );
        assert_eq!(
            policy.exec("keel.version"),
            Some(["/opt/keel/bin/current".to_owned(), "--version".to_owned()].as_slice())
        );
        assert!(matches!(
            Policy::parse("policy:\n  exec_allow: [certbot, { certbot: [certbot, renew] }]\n")
                .unwrap_err(),
            PolicyError::Parse(error) if error.to_string().contains("certbot twice")
        ));
        assert!(matches!(
            Policy::parse("policy:\n  exec_allow: [{ certbot.renew: [] }]\n").unwrap_err(),
            PolicyError::EmptyExecAllow
        ));
        assert!(matches!(
            Policy::parse("policy:\n  exec_allow: [{ certbot.renew: [\"\", renew] }]\n")
                .unwrap_err(),
            PolicyError::EmptyExecAllow
        ));
    }

    #[test]
    fn a_product_section_carries_the_policy_classes_it_is_granted() {
        let policy = Policy::parse(
            "policy:\n  products:\n    demo: { drop_in_prefix: keel-demo-, classes: { note: { root: notes, limit: 3 } } }\n",
        )
        .unwrap();
        let demo = policy.product("demo").unwrap();
        assert_eq!(
            demo.classes.get("note"),
            Some(&serde_json::json!({ "root": "notes", "limit": 3 }))
        );
        assert_eq!(demo.classes.get("vm"), None);
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
            Policy::parse("policy:\n  file_roots: { ftp: /etc/ftp }\n  products: { panel: { user_prefix: hp_, account_roots: [users] } }\n").unwrap_err(),
            PolicyError::AccountRoot { product, root } if product == "panel" && root == "users"
        ));
        assert!(matches!(
            Policy::parse("policy:\n  file_roots: { ftp: /etc/ftp }\n  products: { dns: { drop_in_prefix: keel-dns-, account_roots: [ftp] } }\n").unwrap_err(),
            PolicyError::AccountRoot { product, root } if product == "dns" && root == "ftp"
        ));
        assert_eq!(
            Policy::parse("policy:\n  file_roots: { ftp: /etc/ftp }\n  products: { panel: { user_prefix: hp_, account_roots: [ftp] } }\n")
                .unwrap()
                .product("panel")
                .unwrap()
                .account_roots,
            ["ftp"]
        );
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

    #[test]
    fn a_uid_range_reaching_the_id_that_means_unchanged_is_refused() {
        let error =
            Policy::parse("policy:\n  uid_range: { from: 100000, to: 4294967295 }\n").unwrap_err();
        assert!(matches!(
            error,
            PolicyError::UidRange {
                from: 100_000,
                to: u32::MAX
            }
        ));
        assert_eq!(
            error.to_string(),
            "uid_range 100000..4294967295 is not a range above 0 that stays below 4294967295"
        );
        let policy =
            Policy::parse("policy:\n  uid_range: { from: 100000, to: 4294967294 }\n").unwrap();
        assert!(!policy.uid_range().unwrap().contains(u32::MAX));
    }

    #[test]
    fn a_service_named_like_an_option_never_reaches_systemctl() {
        for name in ["-Hroot@elsewhere", "--force", "-"] {
            let text = format!("policy:\n  services: [\"{name}\"]\n");
            assert!(
                matches!(Policy::parse(&text).unwrap_err(), PolicyError::ServiceName(refused) if refused == name),
                "{name}"
            );
        }
        let policy =
            Policy::parse("policy:\n  services: [nginx, php8.3-fpm, getty@tty1]\n").unwrap();
        assert!(policy.allows_service("getty@tty1"));
    }

    #[test]
    fn a_service_pattern_admits_the_versions_of_one_unit_and_nothing_else() {
        let policy = Policy::parse("policy:\n  services: [nginx, php*-fpm]\n").unwrap();
        for name in ["php8.3-fpm", "php8.4-fpm", "php7-fpm", "nginx"] {
            assert!(policy.allows_service(name), "{name}");
        }
        for name in [
            "php-fpm",
            "phpx-fpm",
            "php8.3-fpm-evil",
            "php8.3;-fpm",
            "php*-fpm",
            "nginx2",
        ] {
            assert!(!policy.allows_service(name), "{name}");
        }
        for entry in ["*", "*-fpm", "php**-fpm", "php*-fpm*", "php*;-fpm"] {
            let text = format!("policy:\n  services: [\"{entry}\"]\n");
            assert!(
                matches!(Policy::parse(&text).unwrap_err(), PolicyError::ServiceName(refused) if refused == entry),
                "{entry}"
            );
        }
    }
}
