use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

use crate::ops::{self, Context};
use crate::policy::{Policy, PolicyError};

pub const SOCKET_MODE: u32 = 0o666;
pub const LINE_LIMIT: usize = 1024 * 1024;

pub const ERROR_FRAME: &str = "host.error.frame";
pub const ERROR_UNKNOWN_OP: &str = "host.error.unknown_op";
pub const ERROR_ARGS: &str = "host.error.args";
pub const ERROR_POLICY: &str = "host.error.policy";
pub const ERROR_RENDER_FAILED: &str = "host.error.render_failed";
pub const ERROR_VALIDATE_FAILED: &str = "host.error.validate_failed";
pub const ERROR_RELOAD_FAILED: &str = "host.error.reload_failed";
pub const ERROR_EXEC_FAILED: &str = "host.error.exec_failed";
pub const ERROR_IO: &str = "host.error.io";

#[derive(Debug, Parser)]
#[command(
    about = "Serve the operations on a unix socket, admitting one uid by its peer credentials"
)]
pub struct Args {
    #[arg(long, help = "The socket to listen on; created with mode 0666")]
    pub socket: PathBuf,
    #[arg(long, help = "The one uid whose connections are admitted")]
    pub uid: u32,
    #[arg(long, help = "The policy file in the shape of agent.yaml")]
    pub policy: PathBuf,
    #[arg(
        long,
        help = "The directory the template names of a request are read from"
    )]
    pub templates: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error("the templates directory {0} is not a directory")]
    Templates(PathBuf),
    #[error("another agent is listening on {0}")]
    Busy(PathBuf),
    #[error("could not bind {path}: {source}")]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not open {path} to every uid: {source}")]
    Permissions {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("waiting for a signal failed: {0}")]
    Signal(#[source] io::Error),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub login: String,
    pub level: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub op: String,
    pub actor: Actor,
    pub request_id: String,
    pub product: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Failure {
    pub key: String,
    pub detail: String,
}

impl Failure {
    pub fn new(key: &str, detail: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Answer {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
}

impl Answer {
    pub fn ok(id: u64, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failed(id: u64, failure: Failure) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(failure),
        }
    }

    pub fn parse(line: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(line)
    }
}

pub fn parse_request(line: &[u8]) -> Result<Request, (u64, Failure)> {
    let request: Request = serde_json::from_slice(line).map_err(|error| {
        let id = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|value| value.get("id").and_then(Value::as_u64))
            .unwrap_or(0);
        (id, Failure::new(ERROR_FRAME, error.to_string()))
    })?;
    let fields = [
        ("op", &request.op),
        ("actor.login", &request.actor.login),
        ("actor.level", &request.actor.level),
        ("request_id", &request.request_id),
        ("product", &request.product),
    ];
    for (name, value) in fields {
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err((
                request.id,
                Failure::new(
                    ERROR_FRAME,
                    format!("{name} carries whitespace or a control character"),
                ),
            ));
        }
    }
    Ok(request)
}

pub fn audit(request: Option<&Request>, outcome: &str) {
    match request {
        Some(request) => eprintln!(
            "audit op={} actor={} level={} request_id={} product={} outcome={outcome}",
            request.op,
            request.actor.login,
            request.actor.level,
            request.request_id,
            request.product
        ),
        None => eprintln!("audit op=- actor=- level=- request_id=- product=- outcome={outcome}"),
    }
}

pub async fn run(args: Args) -> Result<(), AgentError> {
    let policy = Policy::load(&args.policy)?;
    if !args.templates.is_dir() {
        return Err(AgentError::Templates(args.templates));
    }
    let context = Arc::new(Context::new(policy, args.templates.clone()));
    let listener = bind(&args.socket).await?;
    eprintln!(
        "listening socket={} uid={} policy={} templates={}",
        args.socket.display(),
        args.uid,
        args.policy.display(),
        args.templates.display()
    );
    let mut terminate = signal(SignalKind::terminate()).map_err(AgentError::Signal)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(AgentError::Signal)?;
    loop {
        tokio::select! {
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let context = Arc::clone(&context);
                    tokio::spawn(serve(stream, args.uid, context));
                }
                Err(error) => eprintln!("accept failed: {error}"),
            },
        }
    }
    drop(listener);
    if let Err(error) = fs::remove_file(&args.socket)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!(
            "could not remove the socket {}: {error}",
            args.socket.display()
        );
    }
    eprintln!("stopped socket={}", args.socket.display());
    Ok(())
}

async fn bind(path: &Path) -> Result<UnixListener, AgentError> {
    let failed = |source| AgentError::Bind {
        path: path.to_owned(),
        source,
    };
    match fs::symlink_metadata(path) {
        Ok(_) => {
            if UnixStream::connect(path).await.is_ok() {
                return Err(AgentError::Busy(path.to_owned()));
            }
            fs::remove_file(path).map_err(failed)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }
    let listener = UnixListener::bind(path).map_err(failed)?;
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE)).map_err(|source| {
        AgentError::Permissions {
            path: path.to_owned(),
            source,
        }
    })?;
    Ok(listener)
}

async fn serve(stream: UnixStream, admitted: u32, context: Arc<Context>) {
    let peer = match stream.peer_cred() {
        Ok(peer) => peer.uid(),
        Err(error) => {
            eprintln!("refused peer_uid=- admitted_uid={admitted} reason={error}");
            return;
        }
    };
    if peer != admitted {
        eprintln!("refused peer_uid={peer} admitted_uid={admitted}");
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
            audit(None, ERROR_FRAME);
            let _ = send(&mut write, &Answer::failed(0, failure)).await;
            break;
        }
        if payload.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let answer = answer(payload, &context).await;
        if send(&mut write, &answer).await.is_err() {
            break;
        }
    }
}

async fn answer(line: &[u8], context: &Arc<Context>) -> Answer {
    let request = match parse_request(line) {
        Ok(request) => request,
        Err((id, failure)) => {
            audit(None, &failure.key);
            return Answer::failed(id, failure);
        }
    };
    let context = Arc::clone(context);
    let executed = {
        let request = request.clone();
        tokio::task::spawn_blocking(move || ops::execute(&context, &request)).await
    };
    let outcome = executed.unwrap_or_else(|error| {
        Err(Failure::new(
            ERROR_IO,
            format!("the operation did not finish: {error}"),
        ))
    });
    match outcome {
        Ok(data) => {
            audit(Some(&request), "ok");
            Answer::ok(request.id, data)
        }
        Err(failure) => {
            audit(Some(&request), &failure.key);
            Answer::failed(request.id, failure)
        }
    }
}

async fn send(write: &mut OwnedWriteHalf, answer: &Answer) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(answer).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.flush().await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DESIGN_LINE: &str = r#"{"id":1,"op":"host.template.apply","actor":{"login":"alice","level":"user"},"request_id":"req-1a2b","product":"dns","args":{"template":"bind/zone.j2","vars":{"a":1},"dest":{"root":"zones","name":"keel-dns-a.org.zone"},"mode":"0640","owner":"bind","validate":"named-checkzone","reload":"named"}}"#;

    #[test]
    fn the_request_frame_of_the_design_parses_with_its_actor_and_args() {
        let request = parse_request(DESIGN_LINE.as_bytes()).unwrap();
        assert_eq!(request.id, 1);
        assert_eq!(request.op, "host.template.apply");
        assert_eq!(
            request.actor,
            Actor {
                login: "alice".to_owned(),
                level: "user".to_owned()
            }
        );
        assert_eq!(request.request_id, "req-1a2b");
        assert_eq!(request.product, "dns");
        assert_eq!(request.args["dest"]["root"], "zones");
        assert_eq!(request.args["vars"]["a"], 1);
        let echoed = serde_json::to_string(&request).unwrap();
        assert_eq!(parse_request(echoed.as_bytes()).unwrap(), request);
    }

    #[test]
    fn a_line_that_is_not_a_request_fails_as_a_frame_keeping_its_id_when_it_has_one() {
        let (id, failure) = parse_request(br#"{"id":7,"op":"host.user.create"}"#).unwrap_err();
        assert_eq!(id, 7);
        assert_eq!(failure.key, ERROR_FRAME);
        assert!(
            failure.detail.contains("missing field"),
            "{}",
            failure.detail
        );
        let (id, failure) = parse_request(b"not json").unwrap_err();
        assert_eq!(id, 0);
        assert_eq!(failure.key, ERROR_FRAME);
        let (id, _) = parse_request(br#"{"id":"one"}"#).unwrap_err();
        assert_eq!(id, 0);
        let (id, failure) =
            parse_request(br#"{"id":3,"op":"x","actor":{"login":"a","level":"user"},"request_id":"r","product":"demo","extra":1}"#)
                .unwrap_err();
        assert_eq!(id, 3);
        assert!(
            failure.detail.contains("unknown field"),
            "{}",
            failure.detail
        );
        let forged = [
            ("op", "host.user.create outcome=ok"),
            ("actor.login", "alice\naudit op=host.user.create actor=bob"),
            ("actor.level", "user\tadmin"),
            ("request_id", "r 1"),
            ("product", "demo\u{7}"),
        ];
        for (field, value) in forged {
            let mut request = json!({
                "id": 4,
                "op": "x",
                "actor": { "login": "a", "level": "user" },
                "request_id": "r",
                "product": "demo",
            });
            match field {
                "actor.login" => request["actor"]["login"] = json!(value),
                "actor.level" => request["actor"]["level"] = json!(value),
                other => request[other] = json!(value),
            }
            let (id, failure) = parse_request(request.to_string().as_bytes()).unwrap_err();
            assert_eq!(id, 4, "{field}");
            assert_eq!(failure.key, ERROR_FRAME, "{field}");
            assert_eq!(
                failure.detail,
                format!("{field} carries whitespace or a control character")
            );
        }
    }

    #[test]
    fn answers_serialize_to_the_two_shapes_of_the_protocol() {
        let ok = Answer::ok(1, json!({ "changed": true }));
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"id":1,"ok":true,"data":{"changed":true}}"#
        );
        let failed = Answer::failed(
            2,
            Failure::new(ERROR_VALIDATE_FAILED, "nginx: [emerg] unknown directive"),
        );
        assert_eq!(
            serde_json::to_string(&failed).unwrap(),
            r#"{"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] unknown directive"}}"#
        );
        assert_eq!(
            Answer::parse(br#"{"id":1,"ok":true,"data":{"changed":true}}"#).unwrap(),
            ok
        );
        assert_eq!(
            Answer::parse(
                br#"{"id":2,"ok":false,"error":{"key":"host.error.validate_failed","detail":"nginx: [emerg] unknown directive"}}"#
            )
            .unwrap(),
            failed
        );
    }
}
