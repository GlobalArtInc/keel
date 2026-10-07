use std::io::{self, IoSliceMut};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use keel_sdk::prelude::*;
use keel_sdk::system::{PluginLauncher, PluginProcess};
use nix::sys::socket::{ControlMessageOwned, MsgFlags, recvmsg};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Interest};
use tokio::net::UnixStream;
use tokio::time::error::Elapsed;

use crate::api::{HostExecutor, Origin};
use crate::config::Target;
use crate::frame::{self, ERROR_FRAME, ERROR_TIMEOUT, ERROR_UNREACHABLE, failure};

const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;
const HANDED_LIMIT: usize = 64 * 1024;
const CHUNK: usize = 4096;
const PLUGINS: &str = "plugin";
const OP_PLUGIN_ACCOUNT: &str = "host.plugin.account";
const OP_PLUGIN_SPAWN: &str = "host.plugin.spawn";
const OP_PLUGIN_STOP: &str = "host.plugin.stop";

#[derive(Default)]
pub(crate) struct Local {
    target: OnceLock<Target>,
    next: AtomicU64,
}

impl Local {
    pub(crate) fn attach(&self, target: Target) -> bool {
        self.target.set(target).is_ok()
    }

    fn target(&self) -> Result<&Target> {
        self.target.get().ok_or_else(|| {
            failure(
                ERROR_UNREACHABLE,
                "the host module has not started, so no agent socket is known",
            )
        })
    }

    fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }
}

#[async_trait]
impl HostExecutor for Local {
    async fn execute(&self, origin: &Origin, op: &str, args: Value) -> Result<Value> {
        let target = self.target()?;
        let id = self.next_id();
        let line = frame::request(id, origin, op, args)?;
        let answer = settled(
            target,
            op,
            tokio::time::timeout(target.timeout, exchange(&target.socket, &line)).await,
        )?;
        frame::answer(id, &answer)
    }
}

#[async_trait]
impl PluginLauncher for Local {
    async fn account(&self, plugin: &str, request_id: &str) -> Result<()> {
        self.execute(
            &Origin::installation(PLUGINS, request_id),
            OP_PLUGIN_ACCOUNT,
            json!({ "plugin": plugin }),
        )
        .await
        .map(drop)
    }

    async fn spawn(&self, plugin: &str, cmd: &[String], request_id: &str) -> Result<PluginProcess> {
        let target = self.target()?;
        let id = self.next_id();
        let line = frame::request(
            id,
            &Origin::installation(PLUGINS, request_id),
            OP_PLUGIN_SPAWN,
            json!({ "plugin": plugin, "cmd": cmd }),
        )?;
        let (answer, handed) = settled(
            target,
            OP_PLUGIN_SPAWN,
            tokio::time::timeout(target.timeout, handing(&target.socket, &line)).await,
        )?;
        let data = frame::answer(id, &answer)?;
        let pid = data
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| failure(ERROR_FRAME, "the agent answered a spawn without a pid"))?;
        let handed = handed.ok_or_else(|| {
            failure(
                ERROR_FRAME,
                "the agent answered a spawn without the socket of the process",
            )
        })?;
        Ok(PluginProcess {
            pid,
            stdio: std::os::unix::net::UnixStream::from(handed),
        })
    }

    async fn stop(&self, plugin: &str, pid: Option<u32>, request_id: &str) -> Result<Option<i32>> {
        let data = self
            .execute(
                &Origin::installation(PLUGINS, request_id),
                OP_PLUGIN_STOP,
                json!({ "plugin": plugin, "pid": pid }),
            )
            .await?;
        Ok(data
            .get("code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()))
    }
}

fn settled<T>(
    target: &Target,
    op: &str,
    outcome: std::result::Result<io::Result<T>, Elapsed>,
) -> Result<T> {
    match outcome {
        Ok(Ok(answer)) => Ok(answer),
        Ok(Err(error)) => Err(failure(
            ERROR_UNREACHABLE,
            format!("{}: {error}", target.socket.display()),
        )),
        Err(_) => Err(failure(
            ERROR_TIMEOUT,
            format!(
                "{op} got no answer from {} within {}s",
                target.socket.display(),
                target.timeout.as_secs()
            ),
        )),
    }
}

async fn exchange(socket: &Path, line: &[u8]) -> io::Result<Vec<u8>> {
    let stream = UnixStream::connect(socket).await?;
    let (read, mut write) = stream.into_split();
    write.write_all(line).await.map_err(refused)?;
    write.flush().await.map_err(refused)?;
    let mut answer = Vec::new();
    BufReader::new(read)
        .take(ANSWER_LIMIT)
        .read_until(b'\n', &mut answer)
        .await
        .map_err(refused)?;
    match answer.last() {
        Some(b'\n') => Ok(answer),
        None => Err(closed()),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the answer ended without a line break or passed {ANSWER_LIMIT} bytes"),
        )),
    }
}

async fn handing(socket: &Path, line: &[u8]) -> io::Result<(Vec<u8>, Option<OwnedFd>)> {
    let mut stream = UnixStream::connect(socket).await?;
    stream.write_all(line).await.map_err(refused)?;
    stream.flush().await.map_err(refused)?;
    let mut answer = Vec::new();
    let mut handed = None;
    loop {
        stream.readable().await.map_err(refused)?;
        let (chunk, descriptors) = match stream.try_io(Interest::READABLE, || receive(&stream)) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(refused(error)),
        };
        for descriptor in descriptors {
            if handed.replace(descriptor).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the agent handed more than one descriptor",
                ));
            }
        }
        if chunk.is_empty() {
            break;
        }
        answer.extend_from_slice(&chunk);
        if answer.last() == Some(&b'\n') {
            return Ok((answer, handed));
        }
        if answer.len() > HANDED_LIMIT {
            break;
        }
    }
    if answer.is_empty() {
        Err(closed())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the answer ended without a line break or passed {HANDED_LIMIT} bytes"),
        ))
    }
}

fn receive(stream: &UnixStream) -> io::Result<(Vec<u8>, Vec<OwnedFd>)> {
    let mut buffer = [0_u8; CHUNK];
    let mut space = nix::cmsg_space!([RawFd; 2]);
    let mut iov = [IoSliceMut::new(&mut buffer)];
    let message = recvmsg::<()>(
        stream.as_raw_fd(),
        &mut iov,
        Some(&mut space),
        receive_flags(),
    )
    .map_err(io::Error::from)?;
    let mut descriptors = Vec::new();
    for message in message.cmsgs().map_err(io::Error::from)? {
        if let ControlMessageOwned::ScmRights(received) = message {
            for raw in received {
                let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
                close_on_exec(&descriptor)?;
                descriptors.push(descriptor);
            }
        }
    }
    let read = message.bytes;
    Ok((buffer[..read].to_vec(), descriptors))
}

#[cfg(target_os = "linux")]
fn receive_flags() -> MsgFlags {
    MsgFlags::MSG_CMSG_CLOEXEC
}

#[cfg(not(target_os = "linux"))]
fn receive_flags() -> MsgFlags {
    MsgFlags::empty()
}

#[cfg(target_os = "linux")]
fn close_on_exec(_descriptor: &OwnedFd) -> io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn close_on_exec(descriptor: &OwnedFd) -> io::Result<()> {
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};

    fcntl(
        descriptor.as_raw_fd(),
        FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC),
    )
    .map(drop)
    .map_err(io::Error::from)
}

fn refused(error: io::Error) -> io::Error {
    match error.kind() {
        io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe => closed(),
        _ => error,
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the agent closed the connection without an answer; it admits one account by its peer credentials",
    )
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, IoSlice, Write};
    use std::os::unix::net::{UnixListener as StdListener, UnixStream as StdStream};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use keel_sdk::serde_json;
    use keel_sdk::types::ErrorCode;
    use nix::sys::socket::{ControlMessage, sendmsg};
    use tokio::net::UnixListener;

    use super::*;
    use crate::api::testing::origin;

    fn agent(socket: &Path, answer: &'static str, hand: bool) -> JoinHandle<Value> {
        let listener = StdListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            io::BufReader::new(&stream).read_line(&mut line).unwrap();
            let (kernel, plugin) = StdStream::pair().unwrap();
            if hand {
                let descriptors = [kernel.as_raw_fd()];
                sendmsg::<()>(
                    stream.as_raw_fd(),
                    &[IoSlice::new(answer.as_bytes())],
                    &[ControlMessage::ScmRights(&descriptors)],
                    MsgFlags::empty(),
                    None,
                )
                .unwrap();
            } else {
                (&stream).write_all(answer.as_bytes()).unwrap();
            }
            if hand {
                (&plugin).write_all(b"ready\n").unwrap();
            }
            let mut rest = Vec::new();
            let _ = io::Read::read_to_end(&mut &stream, &mut rest);
            drop(kernel);
            serde_json::from_str(&line).unwrap()
        })
    }

    fn target(socket: &Path, seconds: u64) -> Target {
        Target {
            socket: socket.to_owned(),
            timeout: Duration::from_secs(seconds),
        }
    }

    #[tokio::test]
    async fn before_the_start_hook_the_executor_answers_unreachable_without_a_socket() {
        let local = Local::default();
        let error = local
            .execute(&origin(), "host.service.list", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.key(), "host.error.unreachable");
        assert_eq!(error.code(), ErrorCode::Unavailable);
    }

    #[tokio::test]
    async fn a_missing_socket_a_silent_agent_and_a_closed_connection_answer_their_keys() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let missing = Local::default();
        assert!(missing.attach(target(&socket, 5)));
        assert!(!missing.attach(target(&socket, 9)));
        let error = missing
            .execute(&origin(), "host.service.list", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.key(), "host.error.unreachable");
        assert!(
            error.params()["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("agent.sock")),
            "{:?}",
            error.params()
        );

        let listener = UnixListener::bind(&socket).unwrap();
        let held = tokio::spawn(async move {
            let (first, _) = listener.accept().await.unwrap();
            let (second, _) = listener.accept().await.unwrap();
            drop(second);
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(first);
        });
        let silent = Local::default();
        silent.attach(target(&socket, 1));
        let error = silent
            .execute(&origin(), "host.service.list", json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.key(), "host.error.timeout");
        let closed = silent
            .execute(&origin(), "host.service.list", json!({}))
            .await
            .unwrap_err();
        assert_eq!(closed.key(), "host.error.unreachable");
        assert!(
            closed.params()["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("without an answer")),
            "{:?}",
            closed.params()
        );
        held.abort();
    }

    #[tokio::test]
    async fn a_spawn_hands_back_the_socket_of_the_process_with_its_pid() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let agent = agent(
            &socket,
            "{\"id\":1,\"ok\":true,\"data\":{\"pid\":42}}\n",
            true,
        );
        let local = Local::default();
        local.attach(target(&socket, 5));
        let process = local
            .spawn(
                "acme",
                &["./bin/acme".to_owned(), "--serve".to_owned()],
                "req-7",
            )
            .await
            .unwrap();
        assert_eq!(process.pid, 42);
        let mut line = String::new();
        io::BufReader::new(&process.stdio)
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n");
        let request = agent.join().unwrap();
        assert_eq!(request["op"], "host.plugin.spawn");
        assert_eq!(request["product"], "plugin");
        assert_eq!(
            request["actor"],
            json!({ "login": "keel", "level": "service" })
        );
        assert_eq!(request["request_id"], "req-7");
        assert_eq!(
            request["args"],
            json!({ "plugin": "acme", "cmd": ["./bin/acme", "--serve"] })
        );
    }

    #[tokio::test]
    async fn a_spawn_answered_without_a_socket_or_a_pid_is_a_frame_error_and_a_refusal_keeps_its_key()
     {
        for (answer, hand, key) in [
            (
                "{\"id\":1,\"ok\":true,\"data\":{\"pid\":42}}\n",
                false,
                "host.error.frame",
            ),
            (
                "{\"id\":1,\"ok\":true,\"data\":{}}\n",
                true,
                "host.error.frame",
            ),
            (
                "{\"id\":1,\"ok\":false,\"error\":{\"key\":\"host.error.policy\",\"detail\":\"no\"}}\n",
                false,
                "host.error.policy",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("agent.sock");
            let agent = agent(&socket, answer, hand);
            let local = Local::default();
            local.attach(target(&socket, 5));
            let error = local
                .spawn("acme", &["./bin/acme".to_owned()], "req-1")
                .await
                .unwrap_err();
            assert_eq!(error.key(), key, "{answer}");
            agent.join().unwrap();
        }
    }

    #[tokio::test]
    async fn a_stop_answers_the_code_of_a_process_that_ended_by_itself() {
        for (answer, code) in [
            (
                "{\"id\":1,\"ok\":true,\"data\":{\"stopped\":0,\"code\":3}}\n",
                Some(3),
            ),
            (
                "{\"id\":1,\"ok\":true,\"data\":{\"stopped\":1,\"code\":null}}\n",
                None,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("agent.sock");
            let agent = agent(&socket, answer, false);
            let local = Local::default();
            local.attach(target(&socket, 5));
            assert_eq!(local.stop("acme", Some(9), "req-2").await.unwrap(), code);
            let request = agent.join().unwrap();
            assert_eq!(request["op"], "host.plugin.stop");
            assert_eq!(request["args"], json!({ "plugin": "acme", "pid": 9 }));
        }
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("agent.sock");
        let agent = agent(
            &socket,
            "{\"id\":1,\"ok\":true,\"data\":{\"name\":\"kp_acme\"}}\n",
            false,
        );
        let local = Local::default();
        local.attach(target(&socket, 5));
        local.account("acme", "req-3").await.unwrap();
        let request = agent.join().unwrap();
        assert_eq!(request["op"], "host.plugin.account");
        assert_eq!(request["args"], json!({ "plugin": "acme" }));
    }
}
