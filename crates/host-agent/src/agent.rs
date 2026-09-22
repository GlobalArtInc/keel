use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nix::sys::stat::Mode;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};

use crate::accounts::Accounts;
use crate::journal;
use crate::ops::{self, Context};
use crate::peer;
use crate::policy::Policy;
use crate::protocol::{Answer, ERROR_FRAME, ERROR_IO, Failure, LINE_LIMIT, parse_request};

pub const DEFAULT_TEMPLATES: &str = "/etc/keel/templates";
pub const DEFAULT_USER: &str = "keel";

#[cfg(target_os = "linux")]
const SOCKET_UMASK: Mode = Mode::from_bits_truncate(0o111);
#[cfg(not(target_os = "linux"))]
const SOCKET_MODE: Mode = Mode::from_bits_truncate(0o666);
const SOCKET_WATCH: Duration = Duration::from_secs(2);

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
}

pub struct Agent {
    context: Arc<Context>,
    admitted: u32,
}

pub struct Bound {
    agent: Agent,
    listener: UnixListener,
    socket: PathBuf,
}

impl Agent {
    pub fn new(policy: Policy, admitted: u32, templates: PathBuf) -> Self {
        Self {
            context: Arc::new(Context::new(policy, templates)),
            admitted,
        }
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
        self.admitted
    }

    pub async fn bind(self, socket: &Path) -> Result<Bound, AgentError> {
        let listener = bind(socket).await?;
        tracing::info!(
            socket = %socket.display(),
            admitted_uid = self.admitted,
            templates = %self.context.templates().display(),
            "the agent listens"
        );
        Ok(Bound {
            agent: self,
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
                        let context = Arc::clone(&self.agent.context);
                        tokio::spawn(connection(stream, self.agent.admitted, context));
                    }
                    Err(error) => tracing::warn!(error = %error, "accept failed"),
                },
                _ = watch.tick() => self.rebind_if_gone().await,
            }
        }
        drop(self.listener);
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

async fn connection(stream: UnixStream, admitted: u32, context: Arc<Context>) {
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
    loop {
        line.clear();
        let mut limited = (&mut reader).take(LINE_LIMIT as u64 + 1);
        match limited.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let payload = line.strip_suffix(b"\n").unwrap_or(&line);
        if payload.len() > LINE_LIMIT {
            let failure = Failure::new(ERROR_FRAME, format!("a line is over {LINE_LIMIT} bytes"));
            journal::frame(peer, 0, &failure);
            let _ = send(&mut write, &Answer::failed(0, failure)).await;
            break;
        }
        if payload.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let answer = answer(peer, payload, &context).await;
        if send(&mut write, &answer).await.is_err() {
            break;
        }
    }
}

async fn answer(peer: u32, line: &[u8], context: &Arc<Context>) -> Answer {
    let request = match parse_request(line) {
        Ok(request) => request,
        Err((id, failure)) => {
            journal::frame(peer, id, &failure);
            return Answer::failed(id, failure);
        }
    };
    let started = Instant::now();
    let executed = {
        let context = Arc::clone(context);
        let request = request.clone();
        tokio::task::spawn_blocking(move || ops::execute(&context, &request)).await
    };
    let outcome = executed.unwrap_or_else(|error| {
        Err(Failure::new(
            ERROR_IO,
            format!("the operation did not finish: {error}"),
        ))
    });
    journal::operation(peer, &request, &outcome, started.elapsed());
    match outcome {
        Ok(data) => Answer::ok(request.id, data),
        Err(failure) => Answer::failed(request.id, failure),
    }
}

async fn send(write: &mut OwnedWriteHalf, answer: &Answer) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(answer).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.flush().await
}
