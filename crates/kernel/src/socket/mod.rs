mod client;
mod frame;
mod peer;

use std::fs;
use std::future::Future;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use keel_sdk::KeelError;
use keel_sdk::runtime::Db;
use keel_sdk::system::PrincipalSource;
use keel_types::{Envelope, Kind, Meta, Principal, Via};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

use crate::boot;
use crate::dispatcher::{CallOptions, Dispatcher, download_route_error, generate_request_id};
use crate::grants;
use crate::http::{Projection, acceptable_request_id, catalogue};
use crate::keel::StoredPrincipals;

pub use client::{Client, ClientError};
pub use frame::{FRAME_LIMIT, Frame};
pub use peer::{OWNER_LOGIN, Refused, admit, owner};

const SOCKET_MODE: u32 = 0o600;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("could not bind {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("another kernel is listening on {path}")]
    Busy { path: PathBuf },
    #[error("could not read the owner of {path}: {source}")]
    Owner {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not restrict the permissions of {path}: {source}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("serving the control socket failed: {0}")]
    Serve(#[source] io::Error),
}

#[derive(Clone)]
pub struct Socket {
    dispatcher: Arc<Dispatcher>,
}

struct Bound {
    listener: UnixListener,
    path: PathBuf,
    uid: u32,
}

pub struct Serving {
    path: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), SocketError>>,
}

impl Socket {
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        Self { dispatcher }
    }

    async fn bind(path: &Path) -> Result<Bound, SocketError> {
        let failed = |source| SocketError::Bind {
            path: path.to_owned(),
            source,
        };
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        match fs::symlink_metadata(path) {
            Ok(_) => {
                if UnixStream::connect(path).await.is_ok() {
                    return Err(SocketError::Busy {
                        path: path.to_owned(),
                    });
                }
                tracing::warn!(path = %path.display(), "removing a stale control socket");
                fs::remove_file(path).map_err(failed)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed(error)),
        }
        let listener = UnixListener::bind(path).map_err(failed)?;
        fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE)).map_err(|source| {
            SocketError::Permissions {
                path: path.to_owned(),
                source,
            }
        })?;
        let uid = fs::metadata(path)
            .map_err(|source| SocketError::Owner {
                path: path.to_owned(),
                source,
            })?
            .uid();
        tracing::info!(path = %path.display(), uid, "control socket listening");
        Ok(Bound {
            listener,
            path: path.to_owned(),
            uid,
        })
    }

    async fn serve(
        self,
        bound: Bound,
        shutdown: impl Future<Output = ()>,
    ) -> Result<(), SocketError> {
        let Bound {
            listener,
            path,
            uid,
        } = bound;
        let mut connections = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let socket = self.clone();
                        connections.spawn(async move { socket.handle(stream, uid).await });
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "accepting on the control socket failed");
                        tokio::time::sleep(ACCEPT_BACKOFF).await;
                    }
                },
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        drop(listener);
        if let Err(error) = fs::remove_file(&path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %error, "could not remove the control socket");
        }
        let drained = tokio::time::timeout(DRAIN_TIMEOUT, async {
            while connections.join_next().await.is_some() {}
        })
        .await;
        if drained.is_err() {
            tracing::warn!("control socket connections did not finish; aborting them");
            connections.abort_all();
        }
        tracing::info!(path = %path.display(), "control socket closed");
        Ok(())
    }

    pub async fn start(self, path: &Path) -> Result<Serving, SocketError> {
        let bound = Self::bind(path).await?;
        let path = bound.path.clone();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(self.serve(bound, async move {
            let _ = stopped.await;
        }));
        Ok(Serving {
            path,
            stop: Some(stop),
            task,
        })
    }

    async fn handle(&self, stream: UnixStream, owner_uid: u32) {
        let peer = match stream.peer_cred() {
            Ok(peer) => peer,
            Err(error) => {
                tracing::warn!(error = %error, "the peer credentials could not be read; closing");
                return;
            }
        };
        if let Err(refused) = admit(peer.uid(), owner_uid) {
            tracing::warn!(peer = peer.uid(), owner = owner_uid, %refused, "control socket connection refused");
            return;
        }
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        let mut line = Vec::new();
        loop {
            line.clear();
            let mut limited = (&mut reader).take(FRAME_LIMIT as u64 + 1);
            match limited.read_until(b'\n', &mut line).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(error = %error, "control socket connection ended");
                    break;
                }
            }
            let payload = line.strip_suffix(b"\n").unwrap_or(&line);
            if payload.len() > FRAME_LIMIT {
                let error =
                    KeelError::validation("keel.error.body_too_large").param("limit", FRAME_LIMIT);
                let _ = send(&mut write, &error_envelope(error, &generate_request_id())).await;
                break;
            }
            if payload.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let envelope = self.answer(payload).await;
            if send(&mut write, &envelope).await.is_err() {
                break;
            }
        }
    }

    async fn answer(&self, line: &[u8]) -> Envelope<Value> {
        let frame = match Frame::parse(line) {
            Ok(frame) => frame,
            Err(error) => return error_envelope(error, &generate_request_id()),
        };
        let principal = owner();
        match frame {
            Frame::Call {
                func,
                params,
                request_id,
                idempotency_key,
                run_as,
            } => {
                let request_id = request_id.filter(|id| acceptable_request_id(id));
                if let Some(function) = self.dispatcher.registry().function(&func)
                    && function.def().kind() == Kind::Download
                {
                    return error_envelope(
                        download_route_error(function),
                        &request_id.unwrap_or_else(generate_request_id),
                    );
                }
                let principal = match run_as {
                    Some(login) => match self.run_as(&login).await {
                        Ok(principal) => principal,
                        Err(error) => {
                            return error_envelope(
                                error,
                                &request_id.unwrap_or_else(generate_request_id),
                            );
                        }
                    },
                    None => principal,
                };
                let options = CallOptions {
                    request_id,
                    idempotency_key,
                    ..CallOptions::default()
                };
                self.dispatcher
                    .call(principal, &func, params, options)
                    .await
                    .envelope
            }
            Frame::Describe { name } => {
                let request_id = generate_request_id();
                let registry = self.dispatcher.registry();
                let projection = Projection::new(registry, &principal);
                match registry.function(&name) {
                    Some(function) => match projection.admit(function) {
                        Ok(()) => {
                            let descriptor = projection.project(function);
                            ok_envelope(&descriptor, &request_id, &principal)
                        }
                        Err(error) => error_envelope(error, &request_id),
                    },
                    None => error_envelope(
                        KeelError::not_found("keel.error.function_not_found")
                            .param("function", name),
                        &request_id,
                    ),
                }
            }
            Frame::I18n { lang } => {
                let request_id = generate_request_id();
                let catalogue = catalogue(self.dispatcher.registry(), &lang);
                ok_envelope(&catalogue, &request_id, &principal)
            }
            Frame::Boot => {
                let request_id = generate_request_id();
                match boot::payload(
                    self.dispatcher.registry(),
                    self.dispatcher.db(),
                    &principal,
                    None,
                    None,
                )
                .await
                {
                    Ok(payload) => ok_envelope(&payload, &request_id, &principal),
                    Err(error) => error_envelope(error, &request_id),
                }
            }
        }
    }

    async fn run_as(&self, login: &str) -> Result<Principal, KeelError> {
        let db = Db::from_connection(self.dispatcher.db().clone());
        let Some(mut principal) = StoredPrincipals.by_login(login, &db).await? else {
            return Err(
                KeelError::not_found("keel.error.principal_not_found").param("login", login)
            );
        };
        principal.via = Via::Socket;
        principal.actor = Some(Box::new(owner()));
        grants::overlay(self.dispatcher.registry(), &mut principal, &db).await?;
        Ok(principal)
    }
}

async fn send(write: &mut OwnedWriteHalf, envelope: &Envelope<Value>) -> io::Result<()> {
    let mut text = serde_json::to_vec(envelope).map_err(io::Error::other)?;
    text.push(b'\n');
    write.write_all(&text).await?;
    write.flush().await
}

fn ok_envelope<T: Serialize>(data: &T, request_id: &str, principal: &Principal) -> Envelope<Value> {
    let data = match serde_json::to_value(data) {
        Ok(data) => data,
        Err(error) => {
            return error_envelope(
                KeelError::internal("keel.error.internal").source(error),
                request_id,
            );
        }
    };
    Envelope::ok(
        data,
        Meta {
            request_id: request_id.to_owned(),
            principal: principal.meta().ok(),
        },
    )
}

fn error_envelope(error: KeelError, request_id: &str) -> Envelope<Value> {
    let mut wire = error.to_call_error();
    wire.request_id = Some(request_id.to_owned());
    Envelope::err(wire)
}

impl Serving {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn stop(mut self) -> Result<(), SocketError> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.await {
            Ok(outcome) => outcome,
            Err(error) => Err(SocketError::Serve(io::Error::other(error))),
        }
    }
}
