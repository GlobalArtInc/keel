use std::fs;
use std::future::Future;
use std::io::{self, IoSlice};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::sys::socket::{ControlMessage, MsgFlags, sendmsg};
use nix::sys::stat::Mode;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Interest};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};

use crate::accounts::Accounts;
use crate::journal;
use crate::ops::{self, Context, Operation, Template, plugin};
use crate::peer;
use crate::policy::Policy;
use crate::protocol::{Answer, ERROR_FRAME, ERROR_IO, Failure, LINE_LIMIT, Request, parse_request};

pub const DEFAULT_TEMPLATES: &str = "/etc/keel/templates";
pub const DEFAULT_USER: &str = "keel";

#[cfg(target_os = "linux")]
const SOCKET_UMASK: Mode = Mode::from_bits_truncate(0o111);
#[cfg(not(target_os = "linux"))]
const SOCKET_MODE: Mode = Mode::from_bits_truncate(0o666);
const SOCKET_WATCH: Duration = Duration::from_secs(2);
#[cfg(target_os = "linux")]
const EXIT_POLL: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("the user {user} is not an account of this host")]
    UnknownUser { user: String },
    #[error("could not look up the user {user}: {detail}")]
    Lookup { user: String, detail: String },
    #[error("another agent is listening on {0}")]
    Busy(PathBuf),
    #[error("could not create the directory of {path}: {source}")]
    Directory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not bind {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the operation {0} is contributed twice")]
    OperationTwice(String),
    #[error("the operation {0} takes a name of the built-in catalogue")]
    BuiltIn(String),
    #[error("the configuration template {0} is contributed twice")]
    TemplateTwice(String),
}

pub struct Agent {
    context: Context,
}

pub struct Bound {
    context: Arc<Context>,
    listener: UnixListener,
    socket: PathBuf,
}

impl Agent {
    pub fn new(policy: Policy, admitted: u32, templates: PathBuf) -> Self {
        Self {
            context: Context::new(policy, admitted, templates),
        }
    }

    pub fn extend(
        mut self,
        operations: Vec<Operation>,
        templates: Vec<Template>,
    ) -> Result<Self, AgentError> {
        self.context.extend(operations, templates)?;
        Ok(self)
    }

    pub fn for_user(policy: Policy, user: &str, templates: PathBuf) -> Result<Self, AgentError> {
        let found = Accounts::new(&policy.tools().getent)
            .user(user)
            .map_err(|failure| AgentError::Lookup {
                user: user.to_owned(),
                detail: failure.detail,
            })?;
        let account = found.ok_or_else(|| AgentError::UnknownUser {
            user: user.to_owned(),
        })?;
        Ok(Self::new(policy, account.uid, templates))
    }

    pub fn admitted(&self) -> u32 {
        self.context.admitted()
    }

    pub async fn bind(self, socket: &Path) -> Result<Bound, AgentError> {
        let listener = bind(socket).await?;
        tracing::info!(
            socket = %socket.display(),
            admitted_uid = self.context.admitted(),
            templates = %self.context.templates().display(),
            "the agent listens"
        );
        Ok(Bound {
            context: Arc::new(self.context),
            listener,
            socket: socket.to_owned(),
        })
    }
}

impl Bound {
    pub async fn serve(mut self, shutdown: impl Future<Output = ()>) {
        let mut shutdown = std::pin::pin!(shutdown);
        let mut watch = tokio::time::interval(SOCKET_WATCH);
        watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                accepted = self.listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let context = Arc::clone(&self.context);
                        tokio::spawn(connection(stream, context));
                    }
                    Err(error) => tracing::warn!(error = %error, "accept failed"),
                },
                _ = watch.tick() => self.rebind_if_gone().await,
            }
        }
        drop(self.listener);
        self.context.processes().stop_all();
        if let Err(error) = fs::remove_file(&self.socket)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(socket = %self.socket.display(), error = %error, "could not remove the socket");
        }
        tracing::info!(socket = %self.socket.display(), "the agent stopped");
    }

    async fn rebind_if_gone(&mut self) {
        match fs::symlink_metadata(&self.socket) {
            Ok(meta) if meta.file_type().is_socket() => return,
            Ok(_) | Err(_) => {}
        }
        match bind(&self.socket).await {
            Ok(listener) => {
                self.listener = listener;
                tracing::warn!(socket = %self.socket.display(), "the socket had disappeared; bound again");
            }
            Err(error) => {
                tracing::error!(socket = %self.socket.display(), error = %error, "the socket has disappeared and could not be bound again");
            }
        }
    }
}

async fn bind(path: &Path) -> Result<UnixListener, AgentError> {
    let failed = |source| AgentError::Bind {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        fs::create_dir_all(parent).map_err(|source| AgentError::Directory {
            path: path.to_owned(),
            source,
        })?;
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_socket() && UnixStream::connect(path).await.is_ok() {
                return Err(AgentError::Busy(path.to_owned()));
            }
            fs::remove_file(path).map_err(failed)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }
    let listener = listen(path).map_err(failed)?;
    listener.set_nonblocking(true).map_err(failed)?;
    UnixListener::from_std(listener).map_err(failed)
}

#[cfg(target_os = "linux")]
fn listen(path: &Path) -> io::Result<StdUnixListener> {
    use nix::sched::{CloneFlags, unshare};
    use nix::sys::stat::umask;

    let path = path.to_owned();
    std::thread::spawn(move || {
        unshare(CloneFlags::CLONE_FS)?;
        let previous = umask(SOCKET_UMASK);
        let bound = StdUnixListener::bind(&path);
        umask(previous);
        bound
    })
    .join()
    .map_err(|_| io::Error::other("the thread that binds the socket panicked"))?
}

#[cfg(not(target_os = "linux"))]
fn listen(path: &Path) -> io::Result<StdUnixListener> {
    use nix::sys::stat::{FchmodatFlags, fchmodat};

    let listener = StdUnixListener::bind(path)?;
    fchmodat(None, path, SOCKET_MODE, FchmodatFlags::NoFollowSymlink)?;
    Ok(listener)
}

async fn connection(stream: UnixStream, context: Arc<Context>) {
    let admitted = context.admitted();
    let peer = match peer::uid(&stream) {
        Ok(peer) => peer,
        Err(error) => {
            journal::refused_peer(None, admitted, &error.to_string());
            return;
        }
    };
    if peer != admitted {
        journal::refused_peer(Some(peer), admitted, "not the admitted uid");
        return;
    }
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    let mut held: Option<OwnedFd> = None;
    loop {
        line.clear();
        let mut limited = (&mut reader).take(LINE_LIMIT as u64 + 1);
        match limited.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        drop(held.take());
        let payload = line.strip_suffix(b"\n").unwrap_or(&line);
        if payload.len() > LINE_LIMIT {
            let failure = Failure::new(ERROR_FRAME, format!("a line is over {LINE_LIMIT} bytes"));
            journal::frame(peer, 0, &failure);
            let _ = send(&mut write, &Answer::failed(0, failure), None).await;
            break;
        }
        if payload.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let (answer, launched) = answer(peer, payload, &context).await;
        let descriptor = launched.as_ref().map(|launched| &launched.descriptor);
        if send(&mut write, &answer, descriptor).await.is_err() {
            if let Some(launched) = launched {
                plugin::abandon(&context, launched).await;
            }
            break;
        }
        held = launched.map(|launched| launched.descriptor);
    }
}

async fn answer(
    peer: u32,
    line: &[u8],
    context: &Arc<Context>,
) -> (Answer, Option<plugin::Launched>) {
    let request = match parse_request(line) {
        Ok(request) => request,
        Err((id, failure)) => {
            journal::frame(peer, id, &failure);
            return (Answer::failed(id, failure), None);
        }
    };
    let started = Instant::now();
    let (outcome, launched) = match request.op.as_str() {
        plugin::OP_SPAWN => match spawn(context, &request).await {
            Ok(launched) => (
                Ok(serde_json::json!({ "pid": launched.pid })),
                Some(launched),
            ),
            Err(failure) => (Err(failure), None),
        },
        plugin::OP_STOP => (stop(context, &request).await, None),
        _ => (execute(context, &request).await, None),
    };
    journal::operation(
        peer,
        &request,
        context.class_of(&request.op),
        &outcome,
        started.elapsed(),
    );
    let answer = match outcome {
        Ok(data) => Answer::ok(request.id, data),
        Err(failure) => Answer::failed(request.id, failure),
    };
    (answer, launched)
}

async fn execute(context: &Arc<Context>, request: &Request) -> Result<serde_json::Value, Failure> {
    let executed = {
        let context = Arc::clone(context);
        let request = request.clone();
        tokio::task::spawn_blocking(move || ops::execute(&context, &request)).await
    };
    executed.unwrap_or_else(|error| {
        Err(Failure::new(
            ERROR_IO,
            format!("the operation did not finish: {error}"),
        ))
    })
}

async fn stop(context: &Arc<Context>, request: &Request) -> Result<serde_json::Value, Failure> {
    let (answer, sweep) = plugin::stop(context, request).await?;
    if let Some(plugin) = sweep {
        let context = Arc::clone(context);
        tokio::task::spawn_blocking(move || plugin::sweep(&context, &plugin))
            .await
            .map_err(|error| {
                Failure::new(ERROR_IO, format!("the sweep did not finish: {error}"))
            })??;
    }
    Ok(answer)
}

#[cfg(target_os = "linux")]
pub(crate) async fn exited(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid};
    use nix::unistd::Pid;
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let mut children = signal(SignalKind::child()).ok();
    let mut tick = tokio::time::interval(EXIT_POLL);
    loop {
        match waitid(
            Id::Pid(Pid::from_raw(raw)),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOWAIT | WaitPidFlag::WNOHANG,
        ) {
            Ok(WaitStatus::StillAlive) | Err(Errno::EINTR) => {}
            Ok(_) => return true,
            Err(_) => return false,
        }
        match children.as_mut() {
            Some(children) => {
                tokio::select! {
                    _ = children.recv() => {}
                    _ = tick.tick() => {}
                }
            }
            None => {
                tick.tick().await;
            }
        }
    }
}

async fn spawn(context: &Arc<Context>, request: &Request) -> Result<plugin::Launched, Failure> {
    let prepared = {
        let context = Arc::clone(context);
        let request = request.clone();
        tokio::task::spawn_blocking(move || plugin::prepare(&context, &request))
            .await
            .map_err(|error| {
                Failure::new(ERROR_IO, format!("the spawn was not prepared: {error}"))
            })??
    };
    plugin::launch(context, prepared)
}

async fn send(
    write: &mut OwnedWriteHalf,
    answer: &Answer,
    handed: Option<&OwnedFd>,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(answer).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let sent = match handed {
        Some(handed) => with_descriptor(write.as_ref(), &bytes, handed).await?,
        None => 0,
    };
    write.write_all(&bytes[sent..]).await?;
    write.flush().await
}

async fn with_descriptor(stream: &UnixStream, bytes: &[u8], handed: &OwnedFd) -> io::Result<usize> {
    let descriptors = [handed.as_raw_fd()];
    loop {
        stream.writable().await?;
        let sent = stream.try_io(Interest::WRITABLE, || {
            sendmsg::<()>(
                stream.as_raw_fd(),
                &[IoSlice::new(bytes)],
                &[ControlMessage::ScmRights(&descriptors)],
                MsgFlags::empty(),
                None,
            )
            .map_err(io::Error::from)
        });
        match sent {
            Ok(sent) => return Ok(sent),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}
