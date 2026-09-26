use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use nix::fcntl::OFlag;
use serde::Deserialize;
use serde_json::{Value, json};

use super::exec::capture_with;
use super::user::inside;
use super::{Call, Dest};
use crate::home::{
    CHMOD, EXIT_EXISTS, EXIT_IO, EXIT_KIND, EXIT_NOT_FOUND, EXIT_OUTSIDE, EXIT_USAGE, HOME_FLAG,
    LIST, MKDIR, READ, REMOVE, RENAME, SUBCOMMAND, WRITE, parse_mode,
};
use crate::process;
use crate::protocol::{
    ERROR_ARGS, ERROR_EXEC_FAILED, ERROR_EXISTS, ERROR_IO, ERROR_NOT_FOUND, ERROR_POLICY, Failure,
};

const PATH_LIMIT: usize = 4096;
const NAME_LIMIT: usize = 255;
const AGENT_EXEC: &str = "agent-exec";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Folder {
    user: String,
    #[serde(default)]
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Item {
    user: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rename {
    user: String,
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chmod {
    user: String,
    path: String,
    mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Read {
    user: String,
    path: String,
    dest: Dest,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Write {
    user: String,
    path: String,
    source: Dest,
}

struct Account<'a> {
    name: &'a str,
    uid: u32,
    home: PathBuf,
}

#[derive(Clone, Copy)]
enum Follow {
    Link,
    Target,
}

pub(super) fn list(call: &Call<'_>, args: Folder) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let dir = folder(&account, &args.path)?;
    let output = settle(process::run(
        &call.policy.tools().keel,
        worker(call, &account, LIST, &[&dir])?,
    )?)?;
    serde_json::from_slice(&output.stdout)
        .map_err(|error| Failure::new(ERROR_EXEC_FAILED, format!("{SUBCOMMAND} {LIST}: {error}")))
}

pub(super) fn mkdir(call: &Call<'_>, args: Item) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let path = locate(&account, &args.path, Follow::Link)?;
    run(call, &account, MKDIR, &[&path])
}

pub(super) fn rename(call: &Call<'_>, args: Rename) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let from = locate(&account, &args.from, Follow::Link)?;
    let to = locate(&account, &args.to, Follow::Link)?;
    run(call, &account, RENAME, &[&from, &to])
}

pub(super) fn remove(call: &Call<'_>, args: Item) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let path = locate(&account, &args.path, Follow::Link)?;
    run(call, &account, REMOVE, &[&path])
}

pub(super) fn chmod(call: &Call<'_>, args: Chmod) -> Result<Value, Failure> {
    let mode = parse_mode(&args.mode).ok_or_else(|| {
        Failure::args(format!(
            "the mode {} is octal within 0777 without special bits",
            args.mode
        ))
    })?;
    let account = account(call, &args.user)?;
    let path = locate(&account, &args.path, Follow::Target)?;
    let mut operands = worker(call, &account, CHMOD, &[&path])?;
    operands.push(format!("{mode:03o}"));
    settle(process::run(&call.policy.tools().keel, operands)?)?;
    Ok(json!({}))
}

pub(super) fn read(call: &Call<'_>, args: Read) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let path = locate(&account, &args.path, Follow::Target)?;
    let argv = worker(call, &account, READ, &[&path])?;
    let keel = &call.policy.tools().keel;
    let bytes = capture_with(call, &args.dest, false, |file| {
        settle(process::redirected(keel, &argv, Stdio::null(), file)?).map(drop)
    })?;
    Ok(json!({ "bytes": bytes }))
}

pub(super) fn write(call: &Call<'_>, args: Write) -> Result<Value, Failure> {
    let account = account(call, &args.user)?;
    let path = locate(&account, &args.path, Follow::Link)?;
    let staged = call.resolve(&args.source)?;
    let input = staged_file(&staged, call.admitted)?;
    let bytes = input
        .metadata()
        .map_err(|error| Failure::io(staged.display(), &error))?
        .len();
    let argv = worker(call, &account, WRITE, &[&path])?;
    settle(process::redirected(
        &call.policy.tools().keel,
        &argv,
        input,
        Stdio::piped(),
    )?)?;
    Ok(json!({ "bytes": bytes }))
}

fn account<'a>(call: &Call<'_>, name: &'a str) -> Result<Account<'a>, Failure> {
    call.account_name(name)?;
    let range = call.uid_range()?;
    let user = call.accounts().user(name)?.ok_or_else(|| {
        Failure::new(
            ERROR_NOT_FOUND,
            format!("{name} is not an account of this host"),
        )
    })?;
    inside(range, user.uid, name)?;
    inside(range, user.gid, name)?;
    let home = Path::new(&user.home);
    if !home.is_absolute() {
        return Err(Failure::policy(format!(
            "the home {} of {name} is not absolute",
            user.home
        )));
    }
    let home = home
        .canonicalize()
        .map_err(|error| Failure::io(home.display(), &error))?;
    Ok(Account {
        name,
        uid: user.uid,
        home,
    })
}

fn parts(account: &Account<'_>, path: &str) -> Result<Vec<String>, Failure> {
    if path.len() > PATH_LIMIT || path.contains('\0') {
        return Err(Failure::args(format!(
            "a path in the home of {} is at most {PATH_LIMIT} bytes without a NUL",
            account.name
        )));
    }
    if path.starts_with('/') {
        return Err(leaves(account, path));
    }
    let mut parts = Vec::new();
    for part in path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if part == ".." {
            return Err(leaves(account, path));
        }
        if part.len() > NAME_LIMIT {
            return Err(Failure::args(format!(
                "the name {part} is longer than {NAME_LIMIT} bytes"
            )));
        }
        parts.push(part.to_owned());
    }
    Ok(parts)
}

fn leaves(account: &Account<'_>, path: &str) -> Failure {
    Failure::policy(format!("{path} leaves the home of {}", account.name))
}

fn canonical(account: &Account<'_>, path: &str, joined: &Path) -> Result<PathBuf, Failure> {
    let canonical = joined.canonicalize().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Failure::new(
                ERROR_NOT_FOUND,
                format!("{path} does not exist in the home of {}", account.name),
            )
        } else {
            Failure::io(path, &error)
        }
    })?;
    if canonical.starts_with(&account.home) {
        Ok(canonical)
    } else {
        Err(leaves(account, path))
    }
}

fn folder(account: &Account<'_>, path: &str) -> Result<String, Failure> {
    let joined = parts(account, path)?
        .iter()
        .fold(account.home.clone(), |dir, part| dir.join(part));
    text(&canonical(account, path, &joined)?)
}

fn locate(account: &Account<'_>, path: &str, follow: Follow) -> Result<String, Failure> {
    let mut parts = parts(account, path)?;
    let Some(last) = parts.pop() else {
        return Err(Failure::args(format!(
            "the path names no entry below the home of {}",
            account.name
        )));
    };
    let parent = parts
        .iter()
        .fold(account.home.clone(), |dir, part| dir.join(part));
    let entry = canonical(account, path, &parent)?.join(last);
    match follow {
        Follow::Link => text(&entry),
        Follow::Target => text(&canonical(account, path, &entry)?),
    }
}

fn text(path: &Path) -> Result<String, Failure> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::policy(format!("{} is not a UTF-8 path", path.to_string_lossy())))
}

fn worker(
    call: &Call<'_>,
    account: &Account<'_>,
    op: &str,
    operands: &[&str],
) -> Result<Vec<String>, Failure> {
    let keel = text(&call.policy.tools().keel)?;
    let mut argv = vec![
        AGENT_EXEC.to_owned(),
        "--uid".to_owned(),
        account.uid.to_string(),
    ];
    if let Some(policy) = call.policy.source() {
        argv.extend(["--policy".to_owned(), text(policy)?]);
    }
    argv.extend([
        keel,
        SUBCOMMAND.to_owned(),
        HOME_FLAG.to_owned(),
        text(&account.home)?,
        op.to_owned(),
    ]);
    argv.extend(operands.iter().map(|operand| (*operand).to_owned()));
    Ok(argv)
}

fn run(
    call: &Call<'_>,
    account: &Account<'_>,
    op: &str,
    operands: &[&str],
) -> Result<Value, Failure> {
    settle(process::run(
        &call.policy.tools().keel,
        worker(call, account, op, operands)?,
    )?)?;
    Ok(json!({}))
}

fn settle(output: Output) -> Result<Output, Failure> {
    if output.status.success() {
        return Ok(output);
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let key = match output
        .status
        .code()
        .and_then(|code| u8::try_from(code).ok())
    {
        Some(EXIT_NOT_FOUND) => ERROR_NOT_FOUND,
        Some(EXIT_EXISTS) => ERROR_EXISTS,
        Some(EXIT_OUTSIDE) => ERROR_POLICY,
        Some(EXIT_KIND | EXIT_USAGE) => ERROR_ARGS,
        Some(EXIT_IO) => ERROR_IO,
        _ => ERROR_EXEC_FAILED,
    };
    let detail = if detail.is_empty() {
        format!("{SUBCOMMAND} exited with {}", output.status)
    } else {
        detail
    };
    Err(Failure::new(key, detail))
}

fn staged_file(path: &Path, owner: u32) -> Result<File, Failure> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path)
        .map_err(|error| Failure::io(path.display(), &error))?;
    let meta = file
        .metadata()
        .map_err(|error| Failure::io(path.display(), &error))?;
    if !meta.is_file() {
        return Err(Failure::policy(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.uid() != owner {
        return Err(Failure::policy(format!(
            "{} belongs to the uid {}, not to the admitted uid {owner}",
            path.display(),
            meta.uid()
        )));
    }
    Ok(file)
}
