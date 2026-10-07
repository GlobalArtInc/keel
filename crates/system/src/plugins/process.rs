use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, PluginLauncher, PluginProcess};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use super::config::{PluginConfig, Timings};
use super::manifest::{Restart, Run, RunMode};
use crate::notify::api::{Problems, Raise, Severity};

pub const PROBLEM_FAILING: &str = "plugin.failing";
pub const ERROR_FAILING: &str = "plugin.error.failing";
pub const ERROR_STOPPED: &str = "plugin.error.stopped";
pub(super) const PROCESS_SOURCE: &str = "plugin.process";
const FRAME_LIMIT: usize = 1024 * 1024;
const PENDING_LIMIT: usize = 64;
const STOP_WAIT: Duration = Duration::from_secs(5);
const LIVENESS: &str = "ping";
const UNHOSTED: &str =
    "no host agent is composed into this installation, so its process cannot start";

type Pending = Arc<Mutex<BTreeMap<String, oneshot::Sender<Value>>>>;

#[derive(Default)]
pub(super) struct Live(Mutex<BTreeMap<String, Arc<Runtime>>>);

pub(super) struct Runtime {
    plugin: String,
    mode: RunMode,
    cmd: Vec<String>,
    restart: Restart,
    health: Mutex<Health>,
    stopping: watch::Sender<bool>,
    finished: watch::Sender<bool>,
    launcher: OnceLock<Arc<dyn PluginLauncher>>,
    caller: OnceLock<Arc<dyn Caller>>,
}

#[derive(Default)]
struct Health {
    phase: Phase,
    failures: u32,
    opened: Option<Instant>,
    reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Running,
    Open,
    Stopped,
}

enum End {
    Stopping,
    Closed,
    Opened,
    Broken(String),
}

#[derive(Debug, PartialEq, Eq)]
enum Fault {
    Timeout,
    Stuck,
    Closed,
    Busy,
    Start(String),
}

struct Connection {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    closed: Arc<watch::Sender<bool>>,
    reader: JoinHandle<()>,
    next: AtomicU64,
}

impl Runtime {
    pub(super) fn new(plugin: &str, run: &Run) -> Self {
        Self {
            plugin: plugin.to_owned(),
            mode: run.mode,
            cmd: run.cmd.clone(),
            restart: run.restart.unwrap_or(Restart::Always),
            health: Mutex::default(),
            stopping: watch::Sender::new(false),
            finished: watch::Sender::new(false),
            launcher: OnceLock::new(),
            caller: OnceLock::new(),
        }
    }

    fn health(&self) -> MutexGuard<'_, Health> {
        self.health.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn refusal(&self) -> Option<KeelError> {
        let key = match self.health().phase {
            Phase::Open => ERROR_FAILING,
            Phase::Stopped => ERROR_STOPPED,
            Phase::Idle | Phase::Running => return None,
        };
        Some(KeelError::unavailable(key).param("plugin", self.plugin.as_str()))
    }

    fn failing(&self) -> Option<String> {
        let health = self.health();
        match health.phase {
            Phase::Open | Phase::Stopped => health.reason.clone(),
            Phase::Idle | Phase::Running => None,
        }
    }

    pub(super) fn start(self: &Arc<Self>, ctx: &Context) {
        let timings = ctx
            .config::<PluginConfig>()
            .map(|config| config.timings())
            .unwrap_or_else(|_| PluginConfig::default().timings());
        if let Ok(caller) = ctx.require::<dyn Caller>() {
            let _ = self.caller.set(caller);
        }
        let runtime = Arc::clone(self);
        let Ok(launcher) = ctx.require::<dyn PluginLauncher>() else {
            let reason = UNHOSTED.to_owned();
            self.stopped(Some(&reason));
            ctx.spawn(async move {
                runtime.raise(&reason).await;
                runtime.finished.send_replace(true);
            });
            return;
        };
        ctx.spawn(async move {
            runtime.supervise(launcher, timings).await;
            runtime.finished.send_replace(true);
        });
    }

    pub(super) async fn stop(&self) {
        self.stopping.send_replace(true);
        let mut finished = self.finished.subscribe();
        if tokio::time::timeout(STOP_WAIT, finished.wait_for(|done| *done))
            .await
            .is_err()
        {
            tracing::warn!(
                plugin = self.plugin,
                "the supervisor of the plugin did not end in time"
            );
        }
        if let Some(launcher) = self.launcher.get() {
            self.sweep(launcher.as_ref()).await;
        }
    }

    async fn supervise(&self, launcher: Arc<dyn PluginLauncher>, timings: Timings) {
        let _ = self.launcher.set(Arc::clone(&launcher));
        if let Err(error) = launcher.stop(&self.plugin, None, &request_id()).await {
            tracing::debug!(plugin = self.plugin, error = %describe(&error), "the processes of an earlier run were not stopped");
        }
        let mut stopping = self.stopping.subscribe();
        match self.mode {
            RunMode::Daemon => {
                self.daemon(launcher.as_ref(), &timings, &mut stopping)
                    .await
            }
            RunMode::Exec => self.exec(launcher.as_ref(), &timings, &mut stopping).await,
        }
    }

    async fn daemon(
        &self,
        launcher: &dyn PluginLauncher,
        timings: &Timings,
        stopping: &mut watch::Receiver<bool>,
    ) {
        let mut ready = false;
        loop {
            if !self.rest(timings, stopping).await {
                return;
            }
            if !self.prepared(launcher, timings, stopping, &mut ready).await {
                continue;
            }
            let id = request_id();
            let spawned = launcher.spawn(&self.plugin, &self.cmd, &id);
            let Some(spawned) = unless_stopped(stopping, spawned).await else {
                self.sweep(launcher).await;
                return;
            };
            let process = match spawned {
                Ok(process) => process,
                Err(error) => {
                    let reason = format!("its process could not start: {}", describe(&error));
                    self.fault(timings, stopping, reason).await;
                    self.sweep(launcher).await;
                    continue;
                }
            };
            let pid = process.pid;
            let end = match Connection::open(process) {
                Ok(connection) => {
                    self.started(pid);
                    self.watch(&connection, timings, stopping).await
                }
                Err(error) => End::Broken(format!("its socket could not be used: {error}")),
            };
            let target = match end {
                End::Opened => None,
                End::Stopping | End::Closed | End::Broken(_) => Some(pid),
            };
            let code = match launcher.stop(&self.plugin, target, &request_id()).await {
                Ok(code) => code,
                Err(error) => {
                    tracing::warn!(plugin = self.plugin, pid, error = %describe(&error), "the process of the plugin could not be stopped");
                    None
                }
            };
            if matches!(end, End::Stopping) || *stopping.borrow() {
                return;
            }
            match end {
                End::Stopping | End::Opened => {}
                End::Broken(reason) => self.failed(launcher, timings, stopping, reason).await,
                End::Closed => {
                    let reason = match code {
                        Some(code) => format!("its process ended with the code {code}"),
                        None => "its process ended on a signal".to_owned(),
                    };
                    let again = match self.restart {
                        Restart::Always => true,
                        Restart::OnFailure => code != Some(0),
                        Restart::Never => false,
                    };
                    if !again {
                        self.halt((code != Some(0)).then_some(reason)).await;
                        return;
                    }
                    self.failed(launcher, timings, stopping, reason).await;
                }
            }
        }
    }

    async fn watch(
        &self,
        connection: &Connection,
        timings: &Timings,
        stopping: &mut watch::Receiver<bool>,
    ) -> End {
        let since = Instant::now();
        let mut tick = tokio::time::interval(timings.liveness);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = stopped(stopping) => return End::Stopping,
                () = connection.closed() => return End::Closed,
                _ = tick.tick() => {
                    let exchange = connection.exchange(json!({ "t": LIVENESS }), timings.timeout);
                    let Some(exchanged) = unless_stopped(stopping, exchange).await else {
                        return End::Stopping;
                    };
                    match exchanged {
                        Ok(_) => self.succeed(since.elapsed() >= timings.quiet).await,
                        Err(Fault::Closed) => return End::Closed,
                        Err(Fault::Busy) => {}
                        Err(fault @ (Fault::Stuck | Fault::Start(_))) => {
                            return End::Broken(fault.reason(timings));
                        }
                        Err(fault @ Fault::Timeout) => {
                            if self.fault(timings, stopping, fault.reason(timings)).await {
                                return End::Opened;
                            }
                        }
                    }
                }
            }
        }
    }

    async fn prepared(
        &self,
        launcher: &dyn PluginLauncher,
        timings: &Timings,
        stopping: &mut watch::Receiver<bool>,
        ready: &mut bool,
    ) -> bool {
        if *ready {
            return true;
        }
        let id = request_id();
        let account = launcher.account(&self.plugin, &id);
        match unless_stopped(stopping, account).await {
            None => false,
            Some(Ok(())) => {
                *ready = true;
                true
            }
            Some(Err(error)) => {
                let reason = format!("its account could not be prepared: {}", describe(&error));
                self.failed(launcher, timings, stopping, reason).await;
                false
            }
        }
    }

    async fn exec(
        &self,
        launcher: &dyn PluginLauncher,
        timings: &Timings,
        stopping: &mut watch::Receiver<bool>,
    ) {
        let mut ready = false;
        loop {
            if !self.rest(timings, stopping).await {
                return;
            }
            if !self.prepared(launcher, timings, stopping, &mut ready).await {
                continue;
            }
            let once = self.once(launcher, json!({ "t": LIVENESS }), timings);
            match unless_stopped(stopping, once).await {
                None => return,
                Some(Ok(_)) => {
                    self.succeed(true).await;
                    stopped(stopping).await;
                    return;
                }
                Some(Err(fault)) => {
                    self.failed(launcher, timings, stopping, fault.reason(timings))
                        .await;
                }
            }
        }
    }

    async fn once(
        &self,
        launcher: &dyn PluginLauncher,
        frame: Value,
        timings: &Timings,
    ) -> Result<Value, Fault> {
        let process = launcher
            .spawn(&self.plugin, &self.cmd, &request_id())
            .await
            .map_err(|error| Fault::Start(describe(&error)))?;
        let pid = process.pid;
        let answer = match Connection::open(process) {
            Ok(connection) => connection.exchange(frame, timings.timeout).await,
            Err(error) => Err(Fault::Start(error.to_string())),
        };
        if let Err(error) = launcher.stop(&self.plugin, Some(pid), &request_id()).await {
            tracing::warn!(plugin = self.plugin, pid, error = %describe(&error), "the process of a frame could not be stopped");
        }
        answer
    }

    async fn rest(&self, timings: &Timings, stopping: &mut watch::Receiver<bool>) -> bool {
        let pause = {
            let health = self.health();
            match (health.phase, health.opened) {
                (Phase::Open, Some(opened)) => {
                    (opened + timings.quiet).saturating_duration_since(Instant::now())
                }
                _ => timings.pause(health.failures),
            }
        };
        if *stopping.borrow() {
            return false;
        }
        let rested = tokio::select! {
            () = tokio::time::sleep(pause) => true,
            () = stopped(stopping) => false,
        };
        rested && !*stopping.borrow()
    }

    fn started(&self, pid: u32) {
        let mut health = self.health();
        if health.phase != Phase::Open {
            health.phase = Phase::Running;
        }
        tracing::info!(
            plugin = self.plugin,
            pid,
            "the process of the plugin started"
        );
    }

    async fn succeed(&self, stable: bool) {
        let closed = {
            let mut health = self.health();
            let closed = health.phase == Phase::Open;
            health.phase = Phase::Running;
            if stable {
                health.failures = 0;
            }
            health.opened = None;
            health.reason = None;
            closed
        };
        if closed {
            tracing::info!(
                plugin = self.plugin,
                "the plugin answers again; its breaker is closed"
            );
            self.clear().await;
        }
    }

    async fn fault(
        &self,
        timings: &Timings,
        stopping: &watch::Receiver<bool>,
        reason: String,
    ) -> bool {
        if *stopping.borrow() {
            return false;
        }
        self.fail(timings, reason).await
    }

    async fn failed(
        &self,
        launcher: &dyn PluginLauncher,
        timings: &Timings,
        stopping: &watch::Receiver<bool>,
        reason: String,
    ) {
        if self.fault(timings, stopping, reason).await {
            self.sweep(launcher).await;
        }
    }

    async fn sweep(&self, launcher: &dyn PluginLauncher) {
        if let Err(error) = launcher.stop(&self.plugin, None, &request_id()).await {
            tracing::warn!(plugin = self.plugin, error = %describe(&error), "the processes of the plugin could not be stopped");
        }
    }

    async fn fail(&self, timings: &Timings, reason: String) -> bool {
        let (open, opened, failures) = {
            let mut health = self.health();
            health.failures = health.failures.saturating_add(1);
            let was_open = health.phase == Phase::Open;
            let open = was_open || health.failures >= timings.failures;
            if open {
                health.phase = Phase::Open;
                health.opened = Some(Instant::now());
                health.reason = Some(reason.clone());
            }
            (open, open && !was_open, health.failures)
        };
        tracing::warn!(plugin = self.plugin, failures, reason, "the plugin failed");
        if opened {
            tracing::error!(
                plugin = self.plugin,
                reason,
                "the breaker of the plugin is open; its functions answer unavailable until a quiet period passes"
            );
            self.raise(&reason).await;
        }
        open
    }

    fn stopped(&self, reason: Option<&String>) {
        let mut health = self.health();
        health.phase = Phase::Stopped;
        health.reason = reason.cloned();
    }

    async fn halt(&self, reason: Option<String>) {
        self.stopped(reason.as_ref());
        match &reason {
            Some(reason) => {
                tracing::error!(plugin = self.plugin, reason, "the plugin is stopped");
                self.raise(reason).await;
            }
            None => tracing::info!(
                plugin = self.plugin,
                "the process of the plugin ended and is not restarted"
            ),
        }
    }

    async fn raise(&self, reason: &str) {
        let Some(caller) = self.caller.get() else {
            return;
        };
        let raise = match Raise::new(PROBLEM_FAILING, &self.plugin, Severity::Warning)
            .params(json!({ "plugin": self.plugin, "reason": reason }))
        {
            Ok(raise) => raise.source(PROCESS_SOURCE),
            Err(error) => {
                tracing::warn!(plugin = self.plugin, error = %error, "the problem of the plugin could not be built");
                return;
            }
        };
        if let Err(error) = Problems::raise(caller.as_ref(), raise).await {
            tracing::warn!(plugin = self.plugin, error = %describe(&error), "the problem of the plugin could not be raised");
        }
    }

    async fn clear(&self) {
        let Some(caller) = self.caller.get() else {
            return;
        };
        if let Err(error) = Problems::clear(caller.as_ref(), PROBLEM_FAILING, &self.plugin).await {
            tracing::debug!(plugin = self.plugin, error = %describe(&error), "the problem of the plugin was not cleared");
        }
    }
}

impl Live {
    pub(super) fn insert(&self, runtime: &Arc<Runtime>) {
        lock(&self.0).insert(runtime.plugin.clone(), Arc::clone(runtime));
    }

    pub(super) fn failing(&self) -> Vec<(String, String)> {
        lock(&self.0)
            .iter()
            .filter_map(|(plugin, runtime)| {
                runtime.failing().map(|reason| (plugin.clone(), reason))
            })
            .collect()
    }
}

impl Fault {
    fn reason(&self, timings: &Timings) -> String {
        match self {
            Fault::Timeout => format!(
                "a frame got no answer within {} ms",
                timings.timeout.as_millis()
            ),
            Fault::Stuck => format!("it read no frame within {} ms", timings.timeout.as_millis()),
            Fault::Closed => "its process closed its output before it answered".to_owned(),
            Fault::Busy => format!("it holds {PENDING_LIMIT} frames without an answer"),
            Fault::Start(detail) => format!("its process could not start: {detail}"),
        }
    }
}

impl Connection {
    fn open(process: PluginProcess) -> io::Result<Self> {
        process.stdio.set_nonblocking(true)?;
        let (read, write) = UnixStream::from_std(process.stdio)?.into_split();
        let pending = Pending::default();
        let closed = Arc::new(watch::Sender::new(false));
        let reader = tokio::spawn(read_frames(read, Arc::clone(&pending), Arc::clone(&closed)));
        Ok(Self {
            writer: tokio::sync::Mutex::new(write),
            pending,
            closed,
            reader,
            next: AtomicU64::new(0),
        })
    }

    async fn closed(&self) {
        let mut closed = self.closed.subscribe();
        let _ = closed.wait_for(|closed| *closed).await;
    }

    async fn exchange(&self, mut frame: Value, timeout: Duration) -> Result<Value, Fault> {
        if *self.closed.borrow() {
            return Err(Fault::Closed);
        }
        let id = format!("k-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = lock(&self.pending);
            if pending.len() >= PENDING_LIMIT {
                return Err(Fault::Busy);
            }
            pending.insert(id.clone(), sender);
        }
        frame["id"] = json!(id);
        let mut line = serde_json::to_vec(&frame).unwrap_or_default();
        line.push(b'\n');
        let deadline = Instant::now() + timeout;
        let written = tokio::time::timeout_at(deadline, async {
            let mut writer = self.writer.lock().await;
            writer.write_all(&line).await?;
            writer.flush().await
        })
        .await;
        let fault = match written {
            Ok(Ok(())) => None,
            Ok(Err(_)) => Some(Fault::Closed),
            Err(_) => Some(Fault::Stuck),
        };
        if let Some(fault) = fault {
            lock(&self.pending).remove(&id);
            self.closed.send_replace(true);
            return Err(fault);
        }
        match tokio::time::timeout_at(deadline, receiver).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err(Fault::Closed),
            Err(_) => {
                lock(&self.pending).remove(&id);
                Err(Fault::Timeout)
            }
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn read_frames(read: OwnedReadHalf, pending: Pending, closed: Arc<watch::Sender<bool>>) {
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut limited = (&mut reader).take(FRAME_LIMIT as u64 + 1);
        match limited.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.last() != Some(&b'\n') {
            if line.len() > FRAME_LIMIT {
                tracing::warn!(
                    "a plugin wrote a frame over {FRAME_LIMIT} bytes; its output is closed"
                );
            }
            break;
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(id) = frame.get("id").and_then(Value::as_str) else {
            continue;
        };
        let waiting = lock(&pending).remove(id);
        if let Some(waiting) = waiting {
            let _ = waiting.send(frame);
        }
    }
    lock(&pending).clear();
    closed.send_replace(true);
}

async fn stopped(stopping: &mut watch::Receiver<bool>) {
    let _ = stopping.wait_for(|stop| *stop).await;
}

async fn unless_stopped<T>(
    stopping: &mut watch::Receiver<bool>,
    work: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        () = stopped(stopping) => None,
        done = work => Some(done),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn request_id() -> String {
    Id::generate().to_string()
}

fn describe(error: &KeelError) -> String {
    match error.params().get("detail").and_then(Value::as_str) {
        Some(detail) => format!("{}: {detail}", error.key()),
        None => error.key().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream as Socket;

    use super::*;

    #[tokio::test]
    async fn a_plugin_that_reads_no_frame_leaves_the_write_stuck_until_the_timeout() {
        let (kernel, _plugin) = Socket::pair().unwrap();
        let connection = Connection::open(PluginProcess {
            pid: 0,
            stdio: kernel,
        })
        .unwrap();
        let frame = json!({ "t": LIVENESS, "pad": "x".repeat(4 * FRAME_LIMIT) });
        let timeout = Duration::from_millis(200);
        let fault = connection.exchange(frame, timeout).await.unwrap_err();
        assert_eq!(fault, Fault::Stuck);
        let timings = Timings {
            timeout,
            ..PluginConfig::default().timings()
        };
        assert_eq!(fault.reason(&timings), "it read no frame within 200 ms");
        assert!(*connection.closed.borrow());
        assert!(lock(&connection.pending).is_empty());
    }
}
