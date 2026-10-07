use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{Notify, oneshot};

use super::home::{AGENT_EXEC, text};
use super::user::{self, Create, inside};
use super::{Call, Context, parse, valid_account_name};
use crate::accounts::{Accounts, User};
use crate::policy::{Plugins, Product};
use crate::process;
use crate::protocol::{ERROR_EXEC_FAILED, ERROR_NOT_FOUND, Failure, Request};

pub(super) const OP_ACCOUNT: &str = "host.plugin.account";
pub(crate) const OP_SPAWN: &str = "host.plugin.spawn";
pub(crate) const OP_STOP: &str = "host.plugin.stop";

const SECTION: &str = "plugins";
const ID_LIMIT: usize = 64;
const ARG_LIMIT: usize = 64;
const ARG_BYTES: usize = 4096;
const RUNNING_LIMIT: usize = 16;
const ENDED_KEPT: usize = 16;
const STOP_WAIT: Duration = Duration::from_secs(5);
const UID_FLAG: &str = "--uid";
const POLICY_FLAG: &str = "--policy";
const END_OF_OPTIONS: &str = "--";
const SWEEP: [&str; 4] = ["-s", "KILL", "--", "-1"];
const NOTHING_LEFT: &str = "No such process";
const OUTPUT_LINE: usize = 4096;
const OUTPUT_LINES: u32 = 64;
const OUTPUT_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Account {
    plugin: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spawn {
    plugin: String,
    cmd: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stop {
    plugin: String,
    #[serde(default)]
    pid: Option<u32>,
}

pub(crate) struct Prepared {
    plugin: String,
    keel: PathBuf,
    argv: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Processes {
    table: Mutex<Table>,
    changed: Notify,
}

#[derive(Default)]
struct Table {
    entries: BTreeMap<u32, Entry>,
    serial: u64,
    output: BTreeMap<String, Budget>,
}

struct Budget {
    since: Instant,
    shown: u32,
    dropped: u64,
}

struct Entry {
    plugin: String,
    serial: u64,
    state: State,
}

enum State {
    Running(Option<oneshot::Sender<()>>),
    Ended(Option<i32>),
}

pub(super) fn account(context: &Context, args: Account) -> Result<Value, Failure> {
    let plugins = section(context)?;
    let name = account_name(plugins, &args.plugin)?;
    let product = Product {
        user_prefix: Some(plugins.user_prefix.clone()),
        ..Product::default()
    };
    let call = Call {
        policy: &context.policy,
        admitted: context.admitted,
        templates: &context.templates,
        compiled: &context.compiled,
        product: &product,
        product_id: SECTION,
    };
    user::create(&call, Create::plain(name))
}

pub(crate) struct Launched {
    pub(crate) plugin: String,
    pub(crate) pid: u32,
    pub(crate) descriptor: OwnedFd,
}

pub(crate) fn launch(context: &Context, prepared: Prepared) -> Result<Launched, Failure> {
    let (kernel, plugin) =
        UnixStream::pair().map_err(|error| Failure::io("a socket pair", &error))?;
    let input = plugin
        .try_clone()
        .map_err(|error| Failure::io("a socket pair", &error))?;
    let mut command = Command::new(&prepared.keel);
    command
        .args(&prepared.argv)
        .env_clear()
        .env("PATH", process::PATH)
        .env("LC_ALL", "C")
        .current_dir("/")
        .stdin(Stdio::from(OwnedFd::from(input)))
        .stdout(Stdio::from(OwnedFd::from(plugin)))
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let pid = context.processes.start(&prepared.plugin, command)?;
    Ok(Launched {
        plugin: prepared.plugin,
        pid,
        descriptor: OwnedFd::from(kernel),
    })
}

pub(crate) async fn abandon(context: &Context, launched: Launched) {
    drop(launched.descriptor);
    tracing::warn!(
        plugin = %launched.plugin,
        pid = launched.pid,
        "the answer of a spawn did not reach the kernel; its process is stopped"
    );
    context
        .processes
        .stop(&launched.plugin, Some(launched.pid))
        .await;
}

pub(crate) async fn stop(
    context: &Context,
    request: &Request,
) -> Result<(Value, Option<String>), Failure> {
    let args: Stop = parse(&request.args)?;
    plugin_id(&args.plugin)?;
    let answer = context.processes.stop(&args.plugin, args.pid).await;
    Ok((answer, args.pid.is_none().then_some(args.plugin)))
}

pub(crate) fn sweep(context: &Context, plugin: &str) -> Result<(), Failure> {
    let Some(plugins) = context.policy.plugins() else {
        return Ok(());
    };
    let name = account_name(plugins, plugin)?;
    let Some(user) = Accounts::new(&context.policy.tools().getent).user(&name)? else {
        return Ok(());
    };
    let mut argv = exec_argv(context, &user)?;
    argv.push(text(&context.policy.tools().kill)?);
    argv.extend(SWEEP.iter().map(|word| (*word).to_owned()));
    let keel = &context.policy.tools().keel;
    let output = process::run(keel, &argv)?;
    if output.status.success() || nothing_left(&output.stderr) {
        return Ok(());
    }
    Err(Failure::new(
        ERROR_EXEC_FAILED,
        format!(
            "the processes of the plugin {plugin} were not swept: {}",
            process::detail(keel, &output)
        ),
    ))
}

fn nothing_left(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr);
    let mut lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .peekable();
    lines.peek().is_some() && lines.all(|line| line.contains(NOTHING_LEFT))
}

fn exec_argv(context: &Context, user: &User) -> Result<Vec<String>, Failure> {
    let range = context
        .policy
        .uid_range()
        .ok_or_else(|| Failure::policy("the policy allots no uid_range to this installation"))?;
    inside(range, user.uid, &user.name)?;
    inside(range, user.gid, &user.name)?;
    let mut argv = vec![
        AGENT_EXEC.to_owned(),
        UID_FLAG.to_owned(),
        user.uid.to_string(),
    ];
    if let Some(policy) = context.policy.source() {
        argv.extend([POLICY_FLAG.to_owned(), text(policy)?]);
    }
    argv.push(END_OF_OPTIONS.to_owned());
    Ok(argv)
}

fn section(context: &Context) -> Result<&Plugins, Failure> {
    context
        .policy
        .plugins()
        .ok_or_else(|| Failure::policy("the policy has no plugins section"))
}

fn plugin_id(plugin: &str) -> Result<(), Failure> {
    let mut chars = plugin.chars();
    let valid = plugin.len() <= ID_LIMIT
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(Failure::args(format!("{plugin} is not a plugin id")))
    }
}

fn account_name(plugins: &Plugins, plugin: &str) -> Result<String, Failure> {
    plugin_id(plugin)?;
    let name = format!("{}{plugin}", plugins.user_prefix);
    if valid_account_name(&name, &plugins.user_prefix) {
        Ok(name)
    } else {
        Err(Failure::policy(format!(
            "the account {name} of the plugin {plugin} is not an account name the tools accept"
        )))
    }
}

pub(crate) fn prepare(context: &Context, request: &Request) -> Result<Prepared, Failure> {
    let args: Spawn = parse(&request.args)?;
    let plugins = section(context)?;
    let name = account_name(plugins, &args.plugin)?;
    let user = Accounts::new(&context.policy.tools().getent)
        .user(&name)?
        .ok_or_else(|| {
            Failure::new(
                ERROR_NOT_FOUND,
                format!("{name} is not an account of this host"),
            )
        })?;
    let mut argv = exec_argv(context, &user)?;
    if args.cmd.len() > ARG_LIMIT
        || args
            .cmd
            .iter()
            .any(|arg| arg.len() > ARG_BYTES || arg.contains('\0'))
    {
        return Err(Failure::args(format!(
            "a plugin command has at most {ARG_LIMIT} arguments of at most {ARG_BYTES} bytes without a NUL"
        )));
    }
    let (program, operands) = args
        .cmd
        .split_first()
        .ok_or_else(|| Failure::args("the command of the plugin is empty"))?;
    let home = directory(plugins, &args.plugin)?;
    let program = locate(&home, program)?;
    argv.push(text(&program)?);
    argv.extend(operands.iter().cloned());
    Ok(Prepared {
        plugin: args.plugin,
        keel: context.policy.tools().keel.clone(),
        argv,
    })
}

fn directory(plugins: &Plugins, plugin: &str) -> Result<PathBuf, Failure> {
    let root = plugins.root.canonicalize().map_err(|error| {
        Failure::policy(format!(
            "the plugins root {} is not reachable: {error}",
            plugins.root.display()
        ))
    })?;
    let home = root.join(plugin).canonicalize().map_err(|error| {
        Failure::new(
            ERROR_NOT_FOUND,
            format!(
                "the plugin {plugin} has no directory under {}: {error}",
                root.display()
            ),
        )
    })?;
    if home.parent() != Some(root.as_path()) || !home.is_dir() {
        return Err(Failure::policy(format!(
            "the directory of the plugin {plugin} resolves outside {}",
            root.display()
        )));
    }
    Ok(home)
}

fn locate(home: &Path, program: &str) -> Result<PathBuf, Failure> {
    let path = Path::new(program);
    let relative = path
        .components()
        .enumerate()
        .all(|(index, component)| match component {
            Component::Normal(_) => true,
            Component::CurDir => index == 0,
            _ => false,
        })
        && path
            .components()
            .any(|component| matches!(component, Component::Normal(_)));
    if !relative {
        return Err(Failure::policy(format!(
            "the program {program} is not a path inside the plugin directory"
        )));
    }
    let found = home.join(path).canonicalize().map_err(|error| {
        Failure::new(
            ERROR_NOT_FOUND,
            format!("the program {program} is not in the plugin directory: {error}"),
        )
    })?;
    if !found.starts_with(home) || !found.is_file() {
        return Err(Failure::policy(format!(
            "the program {program} resolves outside the plugin directory or is not a file"
        )));
    }
    Ok(found)
}

impl Processes {
    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(self: &Arc<Self>, plugin: &str, mut command: Command) -> Result<u32, Failure> {
        let mut table = self.lock();
        let running = table
            .entries
            .values()
            .filter(|entry| entry.plugin == plugin && matches!(entry.state, State::Running(_)))
            .count();
        if running >= RUNNING_LIMIT {
            return Err(Failure::policy(format!(
                "the plugin {plugin} already runs {RUNNING_LIMIT} processes"
            )));
        }
        let mut child = command.spawn().map_err(|error| {
            Failure::new(
                ERROR_EXEC_FAILED,
                format!("could not start the plugin {plugin}: {error}"),
            )
        })?;
        drop(command);
        let Some(pid) = child.id() else {
            return Err(Failure::new(
                ERROR_EXEC_FAILED,
                format!("the process of the plugin {plugin} ended before it was tracked"),
            ));
        };
        let (kill, killed) = oneshot::channel();
        table.serial += 1;
        let serial = table.serial;
        table.entries.insert(
            pid,
            Entry {
                plugin: plugin.to_owned(),
                serial,
                state: State::Running(Some(kill)),
            },
        );
        drop(table);
        if let Some(stderr) = child.stderr.take() {
            let processes = Arc::clone(self);
            let plugin = plugin.to_owned();
            tokio::spawn(async move { processes.drain(&plugin, pid, stderr).await });
        }
        let processes = Arc::clone(self);
        tokio::spawn(async move { processes.reap(pid, child, killed).await });
        Ok(pid)
    }

    async fn drain(&self, plugin: &str, pid: u32, stderr: ChildStderr) {
        let mut reader = BufReader::with_capacity(OUTPUT_LINE, stderr);
        let mut line = Vec::with_capacity(OUTPUT_LINE);
        let mut cut = false;
        loop {
            let buffer = match reader.fill_buf().await {
                Ok(buffer) if !buffer.is_empty() => buffer,
                _ => break,
            };
            let end = buffer.iter().position(|byte| *byte == b'\n');
            let chunk = &buffer[..end.unwrap_or(buffer.len())];
            let room = OUTPUT_LINE - line.len();
            cut |= chunk.len() > room;
            line.extend_from_slice(&chunk[..chunk.len().min(room)]);
            let consumed = end.map_or(buffer.len(), |end| end + 1);
            reader.consume(consumed);
            if end.is_some() {
                self.show(plugin, pid, &line, cut);
                line.clear();
                cut = false;
            }
        }
        if !line.is_empty() {
            self.show(plugin, pid, &line, cut);
        }
        let dropped = self
            .lock()
            .output
            .get_mut(plugin)
            .map_or(0, |budget| std::mem::take(&mut budget.dropped));
        dropped_output(plugin, dropped);
    }

    fn show(&self, plugin: &str, pid: u32, line: &[u8], cut: bool) {
        let (shown, dropped) = {
            let mut table = self.lock();
            let now = Instant::now();
            let budget = table
                .output
                .entry(plugin.to_owned())
                .or_insert_with(|| Budget {
                    since: now,
                    shown: 0,
                    dropped: 0,
                });
            let mut dropped = 0;
            if now.duration_since(budget.since) >= OUTPUT_WINDOW {
                dropped = budget.dropped;
                *budget = Budget {
                    since: now,
                    shown: 0,
                    dropped: 0,
                };
            }
            let shown = budget.shown < OUTPUT_LINES;
            if shown {
                budget.shown += 1;
            } else {
                budget.dropped += 1;
            }
            (shown, dropped)
        };
        dropped_output(plugin, dropped);
        if shown {
            let output = String::from_utf8_lossy(line);
            tracing::info!(plugin = %plugin, pid, cut, stderr = ?output, "plugin output");
        }
    }

    async fn reap(&self, pid: u32, mut child: Child, killed: oneshot::Receiver<()>) {
        let status = settled(pid, &mut child, killed).await;
        let code = status.ok().and_then(|status| status.code());
        self.ended(pid, code);
    }

    fn ended(&self, pid: u32, code: Option<i32>) {
        let mut table = self.lock();
        let Some(entry) = table.entries.get_mut(&pid) else {
            return;
        };
        entry.state = State::Ended(code);
        let plugin = entry.plugin.clone();
        let mut ended: Vec<(u64, u32)> = table
            .entries
            .iter()
            .filter(|(_, entry)| entry.plugin == plugin && matches!(entry.state, State::Ended(_)))
            .map(|(pid, entry)| (entry.serial, *pid))
            .collect();
        ended.sort_unstable();
        let surplus = ended.len().saturating_sub(ENDED_KEPT);
        for (_, old) in ended.into_iter().take(surplus) {
            table.entries.remove(&old);
        }
        drop(table);
        self.changed.notify_waiters();
    }

    async fn stop(&self, plugin: &str, pid: Option<u32>) -> Value {
        let targets: Vec<u32> = self
            .lock()
            .entries
            .iter()
            .filter(|(found, entry)| entry.plugin == plugin && pid.is_none_or(|pid| pid == **found))
            .map(|(found, _)| *found)
            .collect();
        let mut stopped = 0_u32;
        let mut code = None;
        for target in targets {
            let ended = {
                let mut table = self.lock();
                match table.entries.get_mut(&target).map(|entry| &mut entry.state) {
                    None => continue,
                    Some(State::Ended(found)) => {
                        let found = *found;
                        table.entries.remove(&target);
                        Some(found)
                    }
                    Some(State::Running(kill)) => {
                        if let Some(kill) = kill.take() {
                            let _ = kill.send(());
                        }
                        None
                    }
                }
            };
            match ended {
                Some(found) => code = found,
                None => {
                    code = self.settle(target).await;
                    stopped += 1;
                }
            }
        }
        json!({ "stopped": stopped, "code": pid.and(code) })
    }

    async fn settle(&self, pid: u32) -> Option<i32> {
        let deadline = tokio::time::Instant::now() + STOP_WAIT;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut table = self.lock();
                match table.entries.get(&pid).map(|entry| &entry.state) {
                    Some(State::Running(_)) => {}
                    Some(State::Ended(code)) => {
                        let code = *code;
                        table.entries.remove(&pid);
                        return code;
                    }
                    None => return None,
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                tracing::warn!(
                    pid,
                    "the plugin process did not end in time after it was killed"
                );
                return None;
            }
        }
    }

    pub(crate) fn stop_all(&self) {
        for entry in self.lock().entries.values_mut() {
            if let State::Running(kill) = &mut entry.state
                && let Some(kill) = kill.take()
            {
                let _ = kill.send(());
            }
        }
    }
}

fn dropped_output(plugin: &str, dropped: u64) {
    if dropped > 0 {
        tracing::warn!(plugin = %plugin, dropped, "plugin output dropped");
    }
}

#[cfg(target_os = "linux")]
async fn settled(
    pid: u32,
    child: &mut Child,
    killed: oneshot::Receiver<()>,
) -> io::Result<ExitStatus> {
    let held = tokio::select! {
        held = crate::agent::exited(pid) => held,
        _ = killed => true,
    };
    if held {
        signal_group(pid);
    }
    child.wait().await
}

#[cfg(not(target_os = "linux"))]
async fn settled(
    pid: u32,
    child: &mut Child,
    killed: oneshot::Receiver<()>,
) -> io::Result<ExitStatus> {
    tokio::select! {
        status = child.wait() => status,
        _ = killed => {
            signal_group(pid);
            child.wait().await
        }
    }
}

fn signal_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugins() -> Plugins {
        Plugins {
            root: PathBuf::from("/var/lib/keel/plugins"),
            user_prefix: "kp_".to_owned(),
        }
    }

    #[test]
    fn a_plugin_account_carries_the_prefix_and_fits_the_account_name_limit() {
        assert_eq!(account_name(&plugins(), "acme").unwrap(), "kp_acme");
        assert_eq!(
            account_name(&plugins(), &"a".repeat(29)).unwrap(),
            format!("kp_{}", "a".repeat(29))
        );
        let failure = account_name(&plugins(), &"a".repeat(30)).unwrap_err();
        assert_eq!(failure.key, "host.error.policy");
        for wrong in ["", "Acme", "1acme", "ac-me", "ac/me", "../x", "acme x"] {
            let failure = account_name(&plugins(), wrong).unwrap_err();
            assert_eq!(failure.key, "host.error.args", "{wrong}");
        }
        assert!(plugin_id(&"a".repeat(64)).is_ok());
        assert!(plugin_id(&"a".repeat(65)).is_err());
    }

    #[test]
    fn the_program_is_a_file_inside_the_plugin_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("acme/bin")).unwrap();
        let home = dir.path().join("acme").canonicalize().unwrap();
        std::fs::write(home.join("bin/acme"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.path().join("outside"), "#!/bin/sh\n").unwrap();
        std::os::unix::fs::symlink(dir.path().join("outside"), home.join("bin/link")).unwrap();
        assert_eq!(locate(&home, "./bin/acme").unwrap(), home.join("bin/acme"));
        assert_eq!(locate(&home, "bin/./acme").unwrap(), home.join("bin/acme"));
        for refused in ["/bin/sh", "../outside", "bin/../../outside", ".", "", "bin"] {
            let failure = locate(&home, refused).unwrap_err();
            assert_eq!(failure.key, "host.error.policy", "{refused}");
        }
        assert_eq!(
            locate(&home, "bin/link").unwrap_err().key,
            "host.error.policy"
        );
        assert_eq!(
            locate(&home, "bin/absent").unwrap_err().key,
            "host.error.not_found"
        );
    }
}
