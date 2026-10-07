mod cron;
mod exec;
mod extension;
mod files;
mod fs;
mod group;
mod home;
mod mariadb;
mod pkg;
pub(crate) mod plugin;
mod quota;
mod seal;
mod service;
mod template;
mod user;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::accounts::Accounts;
use crate::agent::AgentError;
use crate::policy::{Policy, Product, UidRange};
use crate::protocol::{ERROR_UNKNOWN_OP, Failure, Request};

pub use extension::{Operation, OperationCall, Template};

const OP_USER_CREATE: &str = "host.user.create";
const OP_USER_DELETE: &str = "host.user.delete";
const OP_USER_GET: &str = "host.user.get";
const OP_USER_LIST: &str = "host.user.list";
const OP_USER_MODIFY: &str = "host.user.modify";
const OP_GROUP_CREATE: &str = "host.group.create";
const OP_GROUP_DELETE: &str = "host.group.delete";
const OP_GROUP_GET: &str = "host.group.get";
const OP_GROUP_LIST: &str = "host.group.list";
const OP_FS_WRITE: &str = "host.fs.write";
const OP_FS_READ: &str = "host.fs.read";
const OP_FS_REMOVE: &str = "host.fs.remove";
const OP_FS_MKDIR: &str = "host.fs.mkdir";
const OP_FS_LIST: &str = "host.fs.list";
const OP_HOME_LIST: &str = "host.home.list";
const OP_HOME_MKDIR: &str = "host.home.mkdir";
const OP_HOME_RENAME: &str = "host.home.rename";
const OP_HOME_REMOVE: &str = "host.home.remove";
const OP_HOME_CHMOD: &str = "host.home.chmod";
const OP_HOME_READ: &str = "host.home.read";
const OP_HOME_WRITE: &str = "host.home.write";
const OP_SERVICE_RELOAD: &str = "host.service.reload";
const OP_SERVICE_RESTART: &str = "host.service.restart";
const OP_SERVICE_LIST: &str = "host.service.list";
const OP_SERVICE_START: &str = "host.service.start";
const OP_SERVICE_STOP: &str = "host.service.stop";
const OP_SERVICE_STATUS: &str = "host.service.status";
const OP_TEMPLATE_APPLY: &str = "host.template.apply";
const OP_PKG_INSTALL: &str = "host.pkg.install";
const OP_PKG_REMOVE: &str = "host.pkg.remove";
const OP_PKG_GET: &str = "host.pkg.get";
const OP_PKG_REFRESH: &str = "host.pkg.refresh";
const OP_CRON_WRITE: &str = "host.cron.write";
const OP_CRON_REMOVE: &str = "host.cron.remove";
const OP_CRON_LIST: &str = "host.cron.list";
const OP_QUOTA_SET: &str = "host.quota.set";
const OP_QUOTA_LIST: &str = "host.quota.list";
const OP_EXEC: &str = "host.exec";
const OP_MARIADB_EXEC: &str = "mariadb.exec";
const CATALOGUE_PREFIX: &str = "host.";

const NAME_LIMIT: usize = 32;

pub(crate) struct Context {
    policy: Policy,
    admitted: u32,
    templates: PathBuf,
    compiled: BTreeMap<String, Template>,
    operations: BTreeMap<String, Operation>,
    serial: Mutex<()>,
    processes: Arc<plugin::Processes>,
}

struct Call<'a> {
    policy: &'a Policy,
    admitted: u32,
    templates: &'a Path,
    compiled: &'a BTreeMap<String, Template>,
    product: &'a Product,
    product_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dest {
    root: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

impl Context {
    pub(crate) fn new(policy: Policy, admitted: u32, templates: PathBuf) -> Self {
        Self {
            policy,
            admitted,
            templates,
            compiled: BTreeMap::new(),
            operations: BTreeMap::new(),
            serial: Mutex::new(()),
            processes: Arc::default(),
        }
    }

    pub(crate) fn templates(&self) -> &Path {
        &self.templates
    }

    pub(crate) fn admitted(&self) -> u32 {
        self.admitted
    }

    pub(crate) fn extend(
        &mut self,
        operations: Vec<Operation>,
        templates: Vec<Template>,
    ) -> Result<(), AgentError> {
        for operation in operations {
            let name = operation.name().to_owned();
            if name.starts_with(CATALOGUE_PREFIX) || name == OP_MARIADB_EXEC {
                return Err(AgentError::BuiltIn(name));
            }
            if self.operations.insert(name.clone(), operation).is_some() {
                return Err(AgentError::OperationTwice(name));
            }
        }
        for template in templates {
            let id = template.id().to_owned();
            if self.compiled.insert(id.clone(), template).is_some() {
                return Err(AgentError::TemplateTwice(id));
            }
        }
        Ok(())
    }

    pub(crate) fn processes(&self) -> &plugin::Processes {
        &self.processes
    }

    pub(crate) fn class_of(&self, op: &str) -> Option<&str> {
        self.operations.get(op).map(Operation::class)
    }
}

pub(crate) fn execute(context: &Context, request: &Request) -> Result<Value, Failure> {
    if request.op == plugin::OP_ACCOUNT {
        let _serial = context
            .serial
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        return plugin::account(context, parse(&request.args)?);
    }
    let product = context.policy.product(&request.product).ok_or_else(|| {
        Failure::policy(format!(
            "the product {} has no section in the policy",
            request.product
        ))
    })?;
    let call = Call {
        policy: &context.policy,
        admitted: context.admitted,
        templates: &context.templates,
        compiled: &context.compiled,
        product,
        product_id: &request.product,
    };
    let _serial = context
        .serial
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let args = &request.args;
    match request.op.as_str() {
        OP_USER_CREATE => user::create(&call, parse(args)?),
        OP_USER_DELETE => user::delete(&call, parse(args)?),
        OP_USER_GET => user::get(&call, parse(args)?),
        OP_USER_LIST => user::list(&call, parse::<Empty>(args).map(drop)?),
        OP_USER_MODIFY => user::modify(&call, parse(args)?),
        OP_GROUP_CREATE => group::create(&call, parse(args)?),
        OP_GROUP_DELETE => group::delete(&call, parse(args)?),
        OP_GROUP_GET => group::get(&call, parse(args)?),
        OP_GROUP_LIST => group::list(&call, parse::<Empty>(args).map(drop)?),
        OP_FS_WRITE => fs::write(&call, parse(args)?),
        OP_FS_READ => fs::read(&call, parse(args)?),
        OP_FS_REMOVE => fs::remove(&call, parse(args)?),
        OP_FS_MKDIR => fs::mkdir(&call, parse(args)?),
        OP_FS_LIST => fs::list(&call, parse(args)?),
        OP_HOME_LIST => home::list(&call, parse(args)?),
        OP_HOME_MKDIR => home::mkdir(&call, parse(args)?),
        OP_HOME_RENAME => home::rename(&call, parse(args)?),
        OP_HOME_REMOVE => home::remove(&call, parse(args)?),
        OP_HOME_CHMOD => home::chmod(&call, parse(args)?),
        OP_HOME_READ => home::read(&call, parse(args)?),
        OP_HOME_WRITE => home::write(&call, parse(args)?),
        OP_SERVICE_RELOAD => service::reload(&call, parse(args)?),
        OP_SERVICE_RESTART => service::restart(&call, parse(args)?),
        OP_SERVICE_LIST => service::list(&call, parse::<Empty>(args).map(drop)?),
        OP_SERVICE_START => service::start(&call, parse(args)?),
        OP_SERVICE_STOP => service::stop(&call, parse(args)?),
        OP_SERVICE_STATUS => service::status(&call, parse::<Empty>(args).map(drop)?),
        OP_TEMPLATE_APPLY => template::apply(&call, parse(args)?),
        OP_PKG_INSTALL => pkg::install(&call, parse(args)?),
        OP_PKG_REMOVE => pkg::remove(&call, parse(args)?),
        OP_PKG_GET => pkg::get(&call, parse(args)?),
        OP_PKG_REFRESH => pkg::refresh(&call, parse::<Empty>(args).map(drop)?),
        OP_CRON_WRITE => cron::write(&call, parse(args)?),
        OP_CRON_REMOVE => cron::remove(&call, parse(args)?),
        OP_CRON_LIST => cron::list(&call, parse::<Empty>(args).map(drop)?),
        OP_QUOTA_SET => quota::set(&call, parse(args)?),
        OP_QUOTA_LIST => quota::list(&call, parse(args)?),
        OP_EXEC => exec::exec(&call, parse(args)?),
        OP_MARIADB_EXEC => mariadb::exec(&call, parse(args)?),
        other => match context.operations.get(other) {
            Some(operation) => extension::run(&call, operation, args),
            None => Err(Failure::new(
                ERROR_UNKNOWN_OP,
                format!("{other} is not an operation of the catalogue"),
            )),
        },
    }
}

fn parse<T: DeserializeOwned>(value: &Value) -> Result<T, Failure> {
    let value = if value.is_null() {
        Value::Object(Default::default())
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| Failure::args(error.to_string()))
}

impl Call<'_> {
    fn accounts(&self) -> Accounts<'_> {
        Accounts::new(&self.policy.tools().getent)
    }

    fn uid_range(&self) -> Result<UidRange, Failure> {
        self.policy
            .uid_range()
            .ok_or_else(|| Failure::policy("the policy allots no uid_range to this installation"))
    }

    fn user_prefix(&self) -> Result<&str, Failure> {
        self.product.user_prefix.as_deref().ok_or_else(|| {
            Failure::policy(format!(
                "the product {} has no user_prefix in the policy",
                self.product_id
            ))
        })
    }

    fn drop_in_prefix(&self) -> Result<&str, Failure> {
        self.product.drop_in_prefix.as_deref().ok_or_else(|| {
            Failure::policy(format!(
                "the product {} has no drop_in_prefix in the policy",
                self.product_id
            ))
        })
    }

    fn account_name(&self, name: &str) -> Result<(), Failure> {
        let prefix = self.user_prefix()?;
        if valid_account_name(name, prefix) {
            Ok(())
        } else {
            Err(Failure::policy(format!(
                "the name {name} is not an account name under the prefix {prefix}"
            )))
        }
    }

    fn names_accounts(&self, root: &str) -> bool {
        self.product.account_roots.iter().any(|named| named == root)
    }

    fn file_prefix(&self, root: &str) -> Result<&str, Failure> {
        if self.names_accounts(root) {
            self.user_prefix()
        } else {
            self.drop_in_prefix()
        }
    }

    fn resolve(&self, dest: &Dest) -> Result<PathBuf, Failure> {
        let prefix = self.file_prefix(&dest.root)?;
        let root = self.policy.root(&dest.root).ok_or_else(|| {
            Failure::policy(format!("{} is not a file root of the policy", dest.root))
        })?;
        let named = if self.names_accounts(&dest.root) {
            valid_account_name(&dest.name, prefix)
        } else {
            valid_file_name(&dest.name, prefix)
        };
        if !named {
            return Err(Failure::policy(format!(
                "the name {} is not a file name under the prefix {prefix}",
                dest.name
            )));
        }
        let canonical = root.canonicalize().map_err(|error| {
            Failure::policy(format!(
                "the file root {} at {} is not reachable: {error}",
                dest.root,
                root.display()
            ))
        })?;
        plain_target(&canonical, &dest.name)
    }

    fn root_dir(&self, root: &str) -> Result<PathBuf, Failure> {
        let path = self
            .policy
            .root(root)
            .ok_or_else(|| Failure::policy(format!("{root} is not a file root of the policy")))?;
        path.canonicalize().map_err(|error| {
            Failure::policy(format!(
                "the file root {root} at {} is not reachable: {error}",
                path.display()
            ))
        })
    }

    fn check(&self, name: &str) -> Result<&[String], Failure> {
        self.policy
            .check(name)
            .ok_or_else(|| Failure::policy(format!("{name} is not a check of the policy")))
    }

    fn service(&self, name: &str) -> Result<(), Failure> {
        if self.policy.allows_service(name) {
            Ok(())
        } else {
            Err(Failure::policy(format!(
                "{name} is not a service of the policy"
            )))
        }
    }
}

fn plain_target(dir: &Path, name: &str) -> Result<PathBuf, Failure> {
    let target = dir.join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&target)
        && meta.file_type().is_symlink()
    {
        return Err(Failure::policy(format!(
            "{} is a symbolic link, which the agent does not follow",
            target.display()
        )));
    }
    Ok(target)
}

fn valid_account_name(name: &str, prefix: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= NAME_LIMIT
        && name.len() > prefix.len()
        && name.starts_with(prefix)
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_file_name(name: &str, prefix: &str) -> bool {
    name.len() > prefix.len()
        && name.starts_with(prefix)
        && !name.contains('/')
        && !name.contains('\0')
        && !name.starts_with('.')
        && name != "."
        && name != ".."
}

const FILE_MODE_LIMIT: u32 = 0o1777;
const DIR_MODE_LIMIT: u32 = 0o3777;

fn parse_mode(text: &str, limit: u32) -> Option<u32> {
    u32::from_str_radix(text, 8)
        .ok()
        .filter(|mode| *mode <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_carry_the_prefix_and_the_characters_the_tools_accept() {
        assert!(valid_account_name("hp_site", "hp_"));
        assert!(valid_account_name("hp_a-1", "hp_"));
        assert!(!valid_account_name("hp_", "hp_"));
        assert!(!valid_account_name("site", "hp_"));
        assert!(!valid_account_name("hp_Site", "hp_"));
        assert!(!valid_account_name("hp_a b", "hp_"));
        assert!(!valid_account_name("hp_../x", "hp_"));
        assert!(!valid_account_name(
            &format!("hp_{}", "a".repeat(30)),
            "hp_"
        ));
        assert!(valid_account_name("root", "ro"));
    }

    #[test]
    fn file_names_carry_the_prefix_and_no_path_separator() {
        assert!(valid_file_name("keel-dns-a.org.zone", "keel-dns-"));
        assert!(!valid_file_name("keel-dns-", "keel-dns-"));
        assert!(!valid_file_name("a.org.zone", "keel-dns-"));
        assert!(!valid_file_name("keel-dns-../x.zone", "keel-dns-"));
        assert!(!valid_file_name("keel-dns-a/b.zone", "keel-dns-"));
        assert!(!valid_file_name("keel-dns-a\0", "keel-dns-"));
        assert!(!valid_file_name(".keel-dns-a", "keel-dns-"));
        assert!(!valid_file_name("..", ""));
        assert!(!valid_file_name(".", ""));
    }

    #[test]
    fn modes_are_octal_and_carry_no_setuid_bit_nor_a_setgid_bit_on_a_file() {
        assert_eq!(parse_mode("0644", FILE_MODE_LIMIT), Some(0o644));
        assert_eq!(parse_mode("640", FILE_MODE_LIMIT), Some(0o640));
        assert_eq!(parse_mode("1777", FILE_MODE_LIMIT), Some(0o1777));
        assert_eq!(parse_mode("4755", FILE_MODE_LIMIT), None);
        assert_eq!(parse_mode("2755", FILE_MODE_LIMIT), None);
        assert_eq!(parse_mode("2775", DIR_MODE_LIMIT), Some(0o2775));
        assert_eq!(parse_mode("4755", DIR_MODE_LIMIT), None);
        assert_eq!(parse_mode("6775", DIR_MODE_LIMIT), None);
        assert_eq!(parse_mode("0o644", FILE_MODE_LIMIT), None);
        assert_eq!(parse_mode("778", FILE_MODE_LIMIT), None);
        assert_eq!(parse_mode("17777", DIR_MODE_LIMIT), None);
        assert_eq!(parse_mode("", FILE_MODE_LIMIT), None);
    }
}
