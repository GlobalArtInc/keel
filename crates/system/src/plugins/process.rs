use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::serde_json;
use keel_sdk::system::{Caller, CallerOptions, PluginLauncher, PluginProcess, PrincipalSource};
use tokio::sync::{Notify, Semaphore, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};

use super::LOGIN_PREFIX;
use super::config::{PluginConfig, Timings};
use super::frames::{self, BEFORE, CALL, Connection, Fault, Invoking, Request, Scope};
use super::manifest::{Restart, Run, RunMode};
use super::register::ERROR_UNAVAILABLE;
use crate::notify::api::{Problems, Raise, Severity};

pub const PROBLEM_FAILING: &str = "plugin.failing";
pub const ERROR_FAILING: &str = "plugin.error.failing";
pub const ERROR_STOPPED: &str = "plugin.error.stopped";
pub(super) const PROCESS_SOURCE: &str = "plugin.process";
const STOP_WAIT: Duration = Duration::from_secs(5);
const EXEC_LIMIT: usize = 16;
const UNHOSTED: &str =
    "no host agent is composed into this installation, so its process cannot start";

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
    principals: OnceLock<Arc<dyn PrincipalSource>>,
    timings: OnceLock<Timings>,
    connection: Mutex<Option<Arc<Connection>>>,
    slots: Arc<Semaphore>,
    tripped: Notify,
    scope: Scope,
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

struct Stopping {
    done: oneshot::Sender<()>,
    stopped: oneshot::Receiver<()>,
}

impl Stopping {
    async fn finish(self) {
        let Self { done, stopped } = self;
        drop(done);
        let _ = stopped.await;
    }
}

enum End {
    Stopping,
    Closed,
    Opened,
    Broken(String),
}

impl Runtime {
    pub(super) fn new(plugin: &str, run: &Run, scope: Scope) -> Self {
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
            principals: OnceLock::new(),
            timings: OnceLock::new(),
            connection: Mutex::new(None),
            slots: Arc::new(Semaphore::new(EXEC_LIMIT)),
            tripped: Notify::new(),
            scope,
        }
    }

    pub(super) async fn call(&self, func: &str, call: Call<Value>) -> Result<Value> {
        let answer = self.forward(CALL, func, &call).await?;
        frames::answered(&answer, &self.plugin)
    }

    pub(super) async fn before(&self, func: &str, call: Call<Value>) -> Result<Before> {
        let answer = self.forward(BEFORE, func, &call).await?;
        frames::verdict(&answer, &self.plugin)
    }

    pub(super) async fn poll(&self, func: &str, db: &Db) -> Result<Vec<Raise>> {
        let (Some(caller), Some(principals)) = (self.caller.get(), self.principals.get()) else {
            return Err(self.unanswered());
        };
        let login = format!("{LOGIN_PREFIX}{}", self.plugin);
        let Some(principal) = principals.by_login(&login, db).await? else {
            return Err(self.unanswered());
        };
        let data = caller
            .call(principal, func, json!({}), CallerOptions::default())
            .await?;
        let raises: Vec<Raise> =
            serde_json::from_value(data).map_err(|_| frames::malformed(&self.plugin))?;
        let prefix = format!("{}.", self.plugin);
        if raises.iter().any(|raise| !raise.key.starts_with(&prefix)) {
            return Err(frames::malformed(&self.plugin));
        }
        Ok(raises)
    }

    async fn forward(&self, t: &'static str, func: &str, call: &Call<Value>) -> Result<Value> {
        if let Some(refusal) = self.refusal() {
            return Err(refusal);
        }
        let Some(timings) = self.timings.get().copied() else {
            return Err(self.unanswered());
        };
        let request = Request {
            t,
            func: Some(func),
            principal: Some(call.principal()),
            params: Some(call.params()),
        };
        let invoking = Invoking {
            call,
            scope: &self.scope,
        };
        let mut stopping = self.stopping.subscribe();
        let exchange = async {
            match self.mode {
                RunMode::Daemon => match self.current() {
                    Some(connection) => {
                        connection
                            .handle(&request, timings.timeout, &invoking)
                            .await
                    }
                    None => Err(Fault::Closed),
                },
                RunMode::Exec => self.spawned(&request, &timings, &invoking).await,
            }
        };
        let Some(exchanged) = unless_stopped(&mut stopping, exchange).await else {
            return Err(self.refusal().unwrap_or_else(|| self.unanswered()));
        };
        match exchanged {
            Ok(answer) => {
                if self.mode == RunMode::Exec {
                    self.succeed(true).await;
                }
                Ok(answer)
            }
            Err(fault) => Err(self.frame_failed(fault, &timings).await),
        }
    }

    async fn spawned(
        &self,
        request: &Request<'_>,
        timings: &Timings,
        invoking: &Invoking<'_>,
    ) -> std::result::Result<Value, Fault> {
        let deadline = Instant::now() + timings.timeout;
        let (process, stopping) = self.launch(deadline).await?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let answer = match Connection::open(&self.plugin, process, timings.timeout) {
            Ok(connection) => connection.handle(request, remaining, invoking).await,
            Err(error) => Err(Fault::Start(error.to_string())),
        };
        stopping.finish().await;
        answer
    }

    async fn launch(
        &self,
        deadline: Instant,
    ) -> std::result::Result<(PluginProcess, Stopping), Fault> {
        let slot = tokio::time::timeout_at(deadline, Arc::clone(&self.slots).acquire_owned())
            .await
            .map_err(|_| Fault::Busy)?
            .map_err(|_| Fault::Busy)?;
        let Some(launcher) = self.launcher.get().map(Arc::clone) else {
            return Err(Fault::Start(UNHOSTED.to_owned()));
        };
        let plugin = self.plugin.clone();
        let cmd = self.cmd.clone();
        let (handed, taken) = oneshot::channel();
        let (done, finished) = oneshot::channel::<()>();
        let (stop, stopped) = oneshot::channel();
        tokio::spawn(async move {
            let _slot = slot;
            let process = match launcher.spawn(&plugin, &cmd, &request_id()).await {
                Ok(process) => process,
                Err(error) => {
                    let _ = handed.send(Err(Fault::Start(describe(&error))));
                    return;
                }
            };
            let pid = process.pid;
            if handed.send(Ok(process)).is_ok() {
                let _ = finished.await;
            }
            if let Err(error) = launcher.stop(&plugin, Some(pid), &request_id()).await {
                tracing::warn!(plugin, pid, error = %describe(&error), "the process of a frame could not be stopped");
            }
            let _ = stop.send(());
        });
        let process = taken
            .await
            .map_err(|_| Fault::Start("its launch ended without an answer".to_owned()))??;
        Ok((process, Stopping { done, stopped }))
    }

    async fn frame_failed(&self, fault: Fault, timings: &Timings) -> KeelError {
        if *self.stopping.borrow() {
            return self.refusal().unwrap_or_else(|| self.unanswered());
        }
        let counted = match (&fault, self.mode) {
            (Fault::Busy, _) => false,
            (Fault::TooLarge, _) => return frames::too_large(&self.plugin),
            (Fault::Timeout, RunMode::Daemon) => true,
            (_, RunMode::Daemon) => false,
            (_, RunMode::Exec) => true,
        };
        if counted && self.fail(timings, fault.reason(timings)).await {
            if self.mode == RunMode::Exec
                && let Some(launcher) = self.launcher.get()
            {
                self.sweep(launcher.as_ref()).await;
            }
            self.tripped.notify_one();
        }
        self.refusal().unwrap_or_else(|| self.unanswered())
    }

    fn current(&self) -> Option<Arc<Connection>> {
        lock(&self.connection).clone()
    }

    fn unanswered(&self) -> KeelError {
        KeelError::unavailable(ERROR_UNAVAILABLE).param("plugin", self.plugin.as_str())
    }

    fn is_open(&self) -> bool {
        self.health().phase == Phase::Open
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
        let _ = self.timings.set(timings);
        if let Ok(caller) = ctx.require::<dyn Caller>() {
            let _ = self.caller.set(caller);
        }
        if let Ok(principals) = ctx.require::<dyn PrincipalSource>() {
            let _ = self.principals.set(principals);
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
            let end = match Connection::open(&self.plugin, process, timings.timeout) {
                Ok(connection) => {
                    let connection = Arc::new(connection);
                    *lock(&self.connection) = Some(Arc::clone(&connection));
                    self.started(pid);
                    let end = self.watch(&connection, timings, stopping).await;
                    *lock(&self.connection) = None;
                    end
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
                () = self.tripped.notified() => {
                    if self.is_open() {
                        return End::Opened;
                    }
                }
                _ = tick.tick() => {
                    let exchange = connection.ping(timings.timeout);
                    let Some(exchanged) = unless_stopped(stopping, exchange).await else {
                        return End::Stopping;
                    };
                    match exchanged {
                        Ok(_) => self.succeed(since.elapsed() >= timings.quiet).await,
                        Err(Fault::Closed) => return End::Closed,
                        Err(Fault::Busy) => {}
                        Err(fault @ (Fault::Stuck | Fault::Start(_) | Fault::TooLarge)) => {
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
            let once = self.once(timings);
            match unless_stopped(stopping, once).await {
                None => return,
                Some(Ok(_)) => {
                    self.succeed(true).await;
                    if !self.until_tripped(stopping).await {
                        return;
                    }
                }
                Some(Err(fault)) => {
                    self.failed(launcher, timings, stopping, fault.reason(timings))
                        .await;
                }
            }
        }
    }

    async fn until_tripped(&self, stopping: &mut watch::Receiver<bool>) -> bool {
        loop {
            tokio::select! {
                () = stopped(stopping) => return false,
                () = self.tripped.notified() => {
                    if self.is_open() {
                        return true;
                    }
                }
            }
        }
    }

    async fn once(&self, timings: &Timings) -> std::result::Result<Value, Fault> {
        let deadline = Instant::now() + timings.timeout;
        let (process, stopping) = self.launch(deadline).await?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let answer = match Connection::open(&self.plugin, process, timings.timeout) {
            Ok(connection) => connection.ping(remaining).await,
            Err(error) => Err(Fault::Start(error.to_string())),
        };
        stopping.finish().await;
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
    use keel_sdk::runtime::CallBuilder;
    use keel_sdk::sea_orm::DatabaseConnection;
    use keel_sdk::system::PluginProcess;
    use keel_sdk::types::{Principal, Scope as Narrowing, Via};

    use super::*;

    #[derive(Default)]
    struct Silent {
        peers: Mutex<Vec<std::os::unix::net::UnixStream>>,
        stops: Mutex<Vec<Option<u32>>>,
    }

    #[async_trait]
    impl PluginLauncher for Silent {
        async fn account(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }

        async fn spawn(&self, _: &str, _: &[String], _: &str) -> Result<PluginProcess> {
            let (stdio, peer) = std::os::unix::net::UnixStream::pair()
                .map_err(|error| KeelError::internal("plugin.error.unavailable").source(error))?;
            lock(&self.peers).push(peer);
            Ok(PluginProcess { pid: 7, stdio })
        }

        async fn stop(&self, _: &str, pid: Option<u32>, _: &str) -> Result<Option<i32>> {
            lock(&self.stops).push(pid);
            Ok(None)
        }
    }

    fn runtime(mode: RunMode) -> Runtime {
        within(mode, Duration::from_millis(150))
    }

    fn within(mode: RunMode, timeout: Duration) -> Runtime {
        let run = Run {
            mode,
            cmd: vec!["./bin/acme".to_owned()],
            restart: None,
        };
        let runtime = Runtime::new(
            "acme",
            &run,
            Scope::new("acme", Level::User, BTreeMap::new()),
        );
        let timings = Timings {
            timeout,
            ..PluginConfig::default().timings()
        };
        runtime
            .timings
            .set(timings)
            .expect("the timings are set once");
        runtime
    }

    fn call() -> Call<Value> {
        let principal = Principal {
            id: Id::generate(),
            login: "alice".to_owned(),
            level: Level::User,
            account_id: Id::generate(),
            role: "user".to_owned(),
            actor: None,
            scope: Narrowing::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".to_owned(),
            grants: BTreeMap::new(),
        };
        CallBuilder::new(principal, "req-1")
            .read(DatabaseConnection::Disconnected)
            .call(json!({}))
    }

    #[tokio::test]
    async fn an_exec_frame_without_a_free_slot_answers_unavailable_and_counts_no_failure() {
        let runtime = runtime(RunMode::Exec);
        let held = runtime
            .slots
            .acquire_many(u32::try_from(EXEC_LIMIT).expect("a small limit"))
            .await
            .expect("every slot");
        let started = Instant::now();
        let error = runtime.call("acme.sync.list", call()).await.unwrap_err();
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert_eq!(error.code(), keel_sdk::types::ErrorCode::Unavailable);
        assert_eq!(error.key(), ERROR_UNAVAILABLE);
        assert_eq!(runtime.health().failures, 0);
        drop(held);
    }

    #[tokio::test]
    async fn an_exec_frame_in_flight_at_a_kernel_stop_ends_at_once_and_counts_no_failure() {
        let runtime = Arc::new(within(RunMode::Exec, Duration::from_secs(5)));
        let launcher = Arc::new(Silent::default());
        runtime
            .launcher
            .set(launcher.clone())
            .unwrap_or_else(|_| panic!("the launcher is set once"));
        runtime.finished.send_replace(true);
        let started = Instant::now();
        let waiting = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            async move { runtime.call("acme.sync.list", call()).await }
        });
        while lock(&launcher.peers).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        runtime.stop().await;
        let error = waiting.await.expect("the frame task ends").unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(error.key(), ERROR_UNAVAILABLE);
        assert_eq!(runtime.health().failures, 0);
        assert_eq!(runtime.health().phase, Phase::Idle);
    }

    #[tokio::test]
    async fn an_exec_frame_whose_caller_went_away_still_stops_its_process_and_frees_its_slot() {
        let runtime = Arc::new(within(RunMode::Exec, Duration::from_secs(5)));
        let launcher = Arc::new(Silent::default());
        runtime
            .launcher
            .set(launcher.clone())
            .unwrap_or_else(|_| panic!("the launcher is set once"));
        let abandoned = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            async move { runtime.call("acme.sync.list", call()).await }
        });
        while lock(&launcher.peers).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        abandoned.abort();
        assert!(abandoned.await.unwrap_err().is_cancelled());
        let started = Instant::now();
        while runtime.slots.available_permits() < EXEC_LIMIT
            && started.elapsed() < Duration::from_secs(2)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*lock(&launcher.stops), vec![Some(7)]);
        assert_eq!(runtime.slots.available_permits(), EXEC_LIMIT);
        assert_eq!(runtime.health().failures, 0);
    }

    #[tokio::test]
    async fn a_daemon_without_a_running_process_answers_unavailable_at_once() {
        let runtime = runtime(RunMode::Daemon);
        let started = Instant::now();
        let error = runtime.call("acme.sync.list", call()).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_millis(150));
        assert_eq!(error.key(), ERROR_UNAVAILABLE);
        runtime.health().phase = Phase::Open;
        let refused = runtime.before("dns.zone.delete", call()).await.unwrap_err();
        assert_eq!(refused.key(), ERROR_FAILING);
        assert_eq!(runtime.health().failures, 0);
    }
}
