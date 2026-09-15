use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

use minijinja::{Environment, UndefinedBehavior};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::agent::{
    ERROR_ARGS, ERROR_EXEC_FAILED, ERROR_IO, ERROR_POLICY, ERROR_RELOAD_FAILED,
    ERROR_RENDER_FAILED, ERROR_UNKNOWN_OP, ERROR_VALIDATE_FAILED, Failure, Request,
};
use crate::policy::{Policy, Product};

pub const OP_USER_CREATE: &str = "host.user.create";
pub const OP_TEMPLATE_APPLY: &str = "host.template.apply";
pub const DEFAULT_SHELL: &str = "/usr/sbin/nologin";
pub const DEFAULT_MODE: u32 = 0o644;
pub const PASSWD: &str = "/etc/passwd";
pub const USER_NAME_LIMIT: usize = 32;

pub struct Context {
    policy: Policy,
    templates: PathBuf,
    serial: Mutex<()>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserCreate {
    name: String,
    #[serde(default)]
    shell: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateApply {
    template: String,
    #[serde(default)]
    vars: Map<String, Value>,
    dest: Dest,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    validate: Option<String>,
    #[serde(default)]
    reload: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dest {
    root: String,
    name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PasswdEntry {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug)]
struct Previous {
    content: Vec<u8>,
    mode: u32,
    owner: PasswdEntry,
}

impl Context {
    pub fn new(policy: Policy, templates: PathBuf) -> Self {
        Self {
            policy,
            templates,
            serial: Mutex::new(()),
        }
    }
}

pub fn execute(context: &Context, request: &Request) -> Result<Value, Failure> {
    let product = context.policy.product(&request.product).ok_or_else(|| {
        policy_failure(format!(
            "the product {} has no section in the policy",
            request.product
        ))
    })?;
    let _serial = context
        .serial
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match request.op.as_str() {
        OP_USER_CREATE => user_create(product, args(&request.args)?),
        OP_TEMPLATE_APPLY => template_apply(context, product, args(&request.args)?),
        other => Err(Failure::new(
            ERROR_UNKNOWN_OP,
            format!("{other} is not an operation of the spike"),
        )),
    }
}

fn args<T: DeserializeOwned>(value: &Value) -> Result<T, Failure> {
    serde_json::from_value(value.clone())
        .map_err(|error| Failure::new(ERROR_ARGS, error.to_string()))
}

fn policy_failure(detail: impl Into<String>) -> Failure {
    Failure::new(ERROR_POLICY, detail)
}

fn io_failure(what: &str, error: io::Error) -> Failure {
    Failure::new(ERROR_IO, format!("{what}: {error}"))
}

fn user_create(product: &Product, args: UserCreate) -> Result<Value, Failure> {
    let prefix = product
        .user_prefix
        .as_deref()
        .ok_or_else(|| policy_failure("the product has no user_prefix"))?;
    if !valid_user_name(&args.name, prefix) {
        return Err(policy_failure(format!(
            "the user name {} is not a name under the prefix {prefix}",
            args.name
        )));
    }
    if let Some(shell) = &args.shell
        && !Path::new(shell).is_absolute()
    {
        return Err(policy_failure(format!(
            "the shell {shell} is not an absolute path"
        )));
    }
    if let Some(entry) = passwd_entry(&args.name)? {
        return Ok(json!({ "uid": entry.uid, "created": false }));
    }
    let shell = args.shell.as_deref().unwrap_or(DEFAULT_SHELL);
    let output = run("useradd", &["--shell", shell, &args.name])
        .map_err(|error| Failure::new(ERROR_EXEC_FAILED, error))?;
    if !output.status.success() {
        return Err(Failure::new(ERROR_EXEC_FAILED, detail("useradd", &output)));
    }
    match passwd_entry(&args.name)? {
        Some(entry) => Ok(json!({ "uid": entry.uid, "created": true })),
        None => Err(Failure::new(
            ERROR_EXEC_FAILED,
            format!("useradd exited 0 but {} is not in {PASSWD}", args.name),
        )),
    }
}

fn template_apply(
    context: &Context,
    product: &Product,
    args: TemplateApply,
) -> Result<Value, Failure> {
    let policy = &context.policy;
    let prefix = product
        .drop_in_prefix
        .as_deref()
        .ok_or_else(|| policy_failure("the product has no drop_in_prefix"))?;
    let root = policy.root(&args.dest.root).ok_or_else(|| {
        policy_failure(format!(
            "{} is not a file root of the policy",
            args.dest.root
        ))
    })?;
    if !valid_file_name(&args.dest.name, prefix) {
        return Err(policy_failure(format!(
            "the name {} is not a file name under the prefix {prefix}",
            args.dest.name
        )));
    }
    let check = match &args.validate {
        Some(name) => Some(
            policy
                .check(name)
                .ok_or_else(|| policy_failure(format!("{name} is not a check of the policy")))?,
        ),
        None => None,
    };
    if let Some(service) = &args.reload
        && !policy.allows_service(service)
    {
        return Err(policy_failure(format!(
            "{service} is not a service of the policy"
        )));
    }
    if !valid_template_name(&args.template) {
        return Err(policy_failure(format!(
            "the template {} is not a plain file name",
            args.template
        )));
    }
    let mode = match &args.mode {
        Some(text) => parse_mode(text)
            .ok_or_else(|| Failure::new(ERROR_ARGS, format!("the mode {text} is not octal")))?,
        None => DEFAULT_MODE,
    };
    let owner = match &args.owner {
        Some(name) => Some(passwd_entry(name)?.ok_or_else(|| {
            policy_failure(format!("the owner {name} is not a user of this host"))
        })?),
        None => None,
    };
    let rendered = render(&context.templates, &args.template, &args.vars)?;
    let target = root.join(&args.dest.name);
    let previous = Previous::read(&target)?;
    if previous
        .as_ref()
        .is_some_and(|previous| previous.content == rendered.as_bytes())
    {
        return Ok(json!({ "changed": false }));
    }
    place(&target, rendered.as_bytes(), mode, owner)?;
    if let Some(argv) = check
        && let Err(output) = check_passes(argv)
    {
        rollback(&target, previous.as_ref())?;
        return Err(Failure::new(ERROR_VALIDATE_FAILED, output));
    }
    if let Some(service) = &args.reload
        && let Err(output) = reload(service)
    {
        rollback(&target, previous.as_ref())?;
        if let Err(again) = reload(service) {
            eprintln!("the reload of {service} after the rollback failed too: {again}");
        }
        return Err(Failure::new(ERROR_RELOAD_FAILED, output));
    }
    Ok(json!({ "changed": true }))
}

pub fn valid_user_name(name: &str, prefix: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= USER_NAME_LIMIT
        && name.len() > prefix.len()
        && name.starts_with(prefix)
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

pub fn valid_file_name(name: &str, prefix: &str) -> bool {
    name.len() > prefix.len()
        && name.starts_with(prefix)
        && !name.contains('/')
        && !name.contains('\0')
        && !name.starts_with('.')
}

pub fn valid_template_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\0') && !name.starts_with('.')
}

pub fn parse_mode(text: &str) -> Option<u32> {
    u32::from_str_radix(text, 8)
        .ok()
        .filter(|mode| *mode <= 0o7777)
}

pub fn lookup_passwd(text: &str, name: &str) -> Option<PasswdEntry> {
    text.lines().find_map(|line| {
        let mut fields = line.split(':');
        if fields.next()? != name {
            return None;
        }
        fields.next()?;
        let uid = fields.next()?.parse().ok()?;
        let gid = fields.next()?.parse().ok()?;
        Some(PasswdEntry { uid, gid })
    })
}

fn passwd_entry(name: &str) -> Result<Option<PasswdEntry>, Failure> {
    let text = fs::read_to_string(PASSWD).map_err(|error| io_failure(PASSWD, error))?;
    Ok(lookup_passwd(&text, name))
}

pub fn render(templates: &Path, name: &str, vars: &Map<String, Value>) -> Result<String, Failure> {
    let path = templates.join(name);
    let source = fs::read_to_string(&path).map_err(|error| {
        Failure::new(
            ERROR_RENDER_FAILED,
            format!("could not read the template {name}: {error}"),
        )
    })?;
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_keep_trailing_newline(true);
    env.add_template(name, &source)
        .and_then(|()| env.get_template(name)?.render(vars))
        .map_err(|error| Failure::new(ERROR_RENDER_FAILED, format!("{error:#}")))
}

impl Previous {
    fn read(target: &Path) -> Result<Option<Self>, Failure> {
        let what = target.display().to_string();
        let meta = match fs::symlink_metadata(target) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_failure(&what, error)),
        };
        if !meta.is_file() {
            return Err(policy_failure(format!("{what} is not a regular file")));
        }
        let content = fs::read(target).map_err(|error| io_failure(&what, error))?;
        Ok(Some(Self {
            content,
            mode: meta.mode() & 0o7777,
            owner: PasswdEntry {
                uid: meta.uid(),
                gid: meta.gid(),
            },
        }))
    }
}

fn place(
    target: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<PasswdEntry>,
) -> Result<(), Failure> {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.tmp"));
    let placed = write_temporary(&temporary, content, mode, owner)
        .and_then(|()| fs::rename(&temporary, target));
    placed.map_err(|error| {
        let _ = fs::remove_file(&temporary);
        io_failure(&target.display().to_string(), error)
    })
}

fn write_temporary(
    temporary: &Path,
    content: &[u8],
    mode: u32,
    owner: Option<PasswdEntry>,
) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(temporary)?;
    file.write_all(content)?;
    file.sync_all()?;
    drop(file);
    if let Some(owner) = owner {
        chown(temporary, Some(owner.uid), Some(owner.gid))?;
    }
    fs::set_permissions(temporary, fs::Permissions::from_mode(mode))
}

fn rollback(target: &Path, previous: Option<&Previous>) -> Result<(), Failure> {
    match previous {
        Some(previous) => place(
            target,
            &previous.content,
            previous.mode,
            Some(previous.owner),
        ),
        None => fs::remove_file(target)
            .map_err(|error| io_failure(&target.display().to_string(), error)),
    }
}

fn check_passes(argv: &[String]) -> Result<(), String> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| "the check has no command".to_owned())?;
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    let output = run(program, &rest)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(detail(program, &output))
    }
}

fn reload(service: &str) -> Result<(), String> {
    let output = run("systemctl", &["reload", service])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(detail("systemctl", &output))
    }
}

fn run(program: &str, args: &[&str]) -> Result<Output, String> {
    Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("could not run {program}: {error}"))
}

fn detail(program: &str, output: &Output) -> String {
    let text = [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        format!("{program} exited with {}", output.status)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/templates");

    #[test]
    fn user_names_carry_the_prefix_and_the_characters_useradd_accepts() {
        assert!(valid_user_name("kd_spike", "kd_"));
        assert!(valid_user_name("kd_a-1", "kd_"));
        assert!(!valid_user_name("kd_", "kd_"));
        assert!(!valid_user_name("spike", "kd_"));
        assert!(!valid_user_name("kd_Spike", "kd_"));
        assert!(!valid_user_name("kd_a b", "kd_"));
        assert!(!valid_user_name("kd_../x", "kd_"));
        assert!(!valid_user_name(&format!("kd_{}", "a".repeat(30)), "kd_"));
        assert!(valid_user_name("root", "ro"));
    }

    #[test]
    fn file_and_template_names_carry_the_prefix_and_no_path_separator() {
        assert!(valid_file_name("keel-demo-spike.conf", "keel-demo-"));
        assert!(!valid_file_name("keel-demo-", "keel-demo-"));
        assert!(!valid_file_name("spike.conf", "keel-demo-"));
        assert!(!valid_file_name("keel-demo-../x.conf", "keel-demo-"));
        assert!(!valid_file_name("keel-demo-a/b.conf", "keel-demo-"));
        assert!(!valid_file_name("keel-demo-a\0", "keel-demo-"));
        assert!(valid_template_name("vhost.conf.j2"));
        assert!(!valid_template_name("../vhost.conf.j2"));
        assert!(!valid_template_name("nginx/vhost.conf.j2"));
        assert!(!valid_template_name(".hidden"));
        assert!(!valid_template_name(""));
    }

    #[test]
    fn modes_are_octal_and_within_the_permission_bits() {
        assert_eq!(parse_mode("0644"), Some(0o644));
        assert_eq!(parse_mode("640"), Some(0o640));
        assert_eq!(parse_mode("4755"), Some(0o4755));
        assert_eq!(parse_mode("0o644"), None);
        assert_eq!(parse_mode("778"), None);
        assert_eq!(parse_mode("17777"), None);
        assert_eq!(parse_mode(""), None);
    }

    #[test]
    fn a_passwd_line_answers_the_uid_and_gid_of_its_name() {
        let text = "root:x:0:0:root:/root:/bin/bash\nkd_spike:x:1001:1001::/nonexistent:/usr/sbin/nologin\nbroken:x:one\n";
        assert_eq!(
            lookup_passwd(text, "kd_spike"),
            Some(PasswdEntry {
                uid: 1001,
                gid: 1001
            })
        );
        assert_eq!(
            lookup_passwd(text, "root"),
            Some(PasswdEntry { uid: 0, gid: 0 })
        );
        assert_eq!(lookup_passwd(text, "kd_"), None);
        assert_eq!(lookup_passwd(text, "broken"), None);
    }

    #[test]
    fn the_vhost_template_renders_a_server_block_and_a_missing_variable_fails_the_render() {
        let vars = serde_json::from_value(json!({
            "server_name": "spike.example",
            "listen": "80",
            "root": "/var/www/keel-demo-spike",
        }))
        .unwrap();
        let rendered = render(Path::new(TEMPLATES), "vhost.conf.j2", &vars).unwrap();
        assert_eq!(
            rendered,
            "server {\n    listen 80;\n    server_name spike.example;\n    root /var/www/keel-demo-spike;\n    index index.html;\n\n    location / {\n        try_files $uri $uri/ =404;\n    }\n}\n"
        );
        let missing = serde_json::from_value(json!({ "server_name": "spike.example" })).unwrap();
        let failure = render(Path::new(TEMPLATES), "vhost.conf.j2", &missing).unwrap_err();
        assert_eq!(failure.key, ERROR_RENDER_FAILED);
        assert!(failure.detail.contains("listen"), "{}", failure.detail);
        let absent = render(Path::new(TEMPLATES), "other.j2", &vars).unwrap_err();
        assert_eq!(absent.key, ERROR_RENDER_FAILED);
        assert!(absent.detail.contains("other.j2"), "{}", absent.detail);
    }

    #[test]
    fn a_file_is_placed_through_a_temporary_beside_it_and_rolled_back_to_what_was_there() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("keel-demo-spike.conf");
        assert!(Previous::read(&target).unwrap().is_none());
        place(&target, b"one\n", 0o640, None).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one\n");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o640);
        let previous = Previous::read(&target).unwrap().unwrap();
        assert_eq!(previous.content, b"one\n");
        assert_eq!(previous.mode, 0o640);
        place(&target, b"two\n", 0o644, Some(previous.owner)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two\n");
        rollback(&target, Some(&previous)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one\n");
        assert_eq!(fs::metadata(&target).unwrap().mode() & 0o7777, 0o640);
        rollback(&target, None).unwrap();
        assert!(!target.exists());
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
        fs::create_dir(&target).unwrap();
        let failure = Previous::read(&target).unwrap_err();
        assert_eq!(failure.key, ERROR_POLICY);
    }
}
