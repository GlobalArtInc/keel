use std::io;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use keel_sdk::prelude::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::api::{HostExecutor, Origin};
use crate::config::Target;
use crate::frame::{self, ERROR_TIMEOUT, ERROR_UNREACHABLE, failure};

const ANSWER_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Local {
    target: OnceLock<Target>,
    next: AtomicU64,
}

impl Local {
    pub(crate) fn attach(&self, target: Target) -> bool {
        self.target.set(target).is_ok()
    }
}

#[async_trait]
impl HostExecutor for Local {
    async fn execute(&self, origin: &Origin, op: &str, args: Value) -> Result<Value> {
        let target = self.target.get().ok_or_else(|| {
            failure(
                ERROR_UNREACHABLE,
                "the host module has not started, so no agent socket is known",
            )
        })?;
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let line = frame::request(id, origin, op, args)?;
        match tokio::time::timeout(target.timeout, exchange(&target.socket, &line)).await {
            Ok(Ok(answer)) => frame::answer(id, &answer),
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
    use std::time::Duration;

    use keel_sdk::types::ErrorCode;
    use tokio::net::UnixListener;

    use super::*;
    use crate::api::testing::origin;

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
}
