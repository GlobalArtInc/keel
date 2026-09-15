use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::agent::{Actor, Answer, Request};

pub const DEFAULT_LEVEL: &str = "user";
pub const REQUEST_ID_PREFIX: &str = "spike";

#[derive(Debug, Parser)]
#[command(
    about = "Send one operation to the agent, print its answer and exit 1 when it is refused"
)]
pub struct Args {
    #[arg(long, help = "The socket of the agent")]
    pub socket: PathBuf,
    #[arg(long, help = "The operation, host.user.create or host.template.apply")]
    pub op: String,
    #[arg(
        long,
        default_value = "{}",
        help = "The arguments of the operation as JSON"
    )]
    pub args: String,
    #[arg(
        long,
        help = "The login of the principal on whose behalf the call is made"
    )]
    pub actor: String,
    #[arg(long, default_value = DEFAULT_LEVEL, help = "The level of that principal")]
    pub level: String,
    #[arg(long, help = "The product whose policy section applies")]
    pub product: String,
    #[arg(
        long,
        help = "The request id written to the audit line; generated when absent"
    )]
    pub request_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("--args is not JSON: {0}")]
    Args(#[source] serde_json::Error),
    #[error("could not connect to {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the agent closed the connection without an answer")]
    Closed,
    #[error("talking to the agent failed: {0}")]
    Io(#[from] io::Error),
    #[error("the agent answered something that is not an answer: {0}")]
    Answer(#[source] serde_json::Error),
}

pub fn generate_request_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    format!("{REQUEST_ID_PREFIX}-{millis}-{}", std::process::id())
}

pub fn request(args: &Args) -> Result<Request, ClientError> {
    let parsed: Value = serde_json::from_str(&args.args).map_err(ClientError::Args)?;
    Ok(Request {
        id: 1,
        op: args.op.clone(),
        actor: Actor {
            login: args.actor.clone(),
            level: args.level.clone(),
        },
        request_id: args.request_id.clone().unwrap_or_else(generate_request_id),
        product: args.product.clone(),
        args: parsed,
    })
}

pub async fn call(socket: &PathBuf, request: &Request) -> Result<(String, Answer), ClientError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|source| ClientError::Connect {
            path: socket.clone(),
            source,
        })?;
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_vec(request).map_err(ClientError::Answer)?;
    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await?;
    let mut answer = String::new();
    let read = BufReader::new(read).read_line(&mut answer).await?;
    if read == 0 {
        return Err(ClientError::Closed);
    }
    let trimmed = answer.trim_end().to_owned();
    let parsed = Answer::parse(trimmed.as_bytes()).map_err(ClientError::Answer)?;
    Ok((trimmed, parsed))
}

pub async fn run(args: Args) -> Result<bool, ClientError> {
    let request = request(&args)?;
    let (line, answer) = call(&args.socket, &request).await?;
    println!("{line}");
    Ok(answer.ok)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn args(request_id: Option<&str>) -> Args {
        Args {
            socket: PathBuf::from("/run/keel-spike/agent.sock"),
            op: "host.user.create".to_owned(),
            args: r#"{"name":"kd_spike"}"#.to_owned(),
            actor: "keel".to_owned(),
            level: DEFAULT_LEVEL.to_owned(),
            product: "demo".to_owned(),
            request_id: request_id.map(str::to_owned),
        }
    }

    #[test]
    fn the_client_builds_the_request_frame_from_its_flags() {
        let framed = request(&args(Some("req-1"))).unwrap();
        assert_eq!(
            serde_json::to_value(&framed).unwrap(),
            json!({
                "id": 1,
                "op": "host.user.create",
                "actor": { "login": "keel", "level": "user" },
                "request_id": "req-1",
                "product": "demo",
                "args": { "name": "kd_spike" }
            })
        );
        let generated = request(&args(None)).unwrap();
        assert!(
            generated
                .request_id
                .starts_with(&format!("{REQUEST_ID_PREFIX}-")),
            "{}",
            generated.request_id
        );
        let mut broken = args(None);
        broken.args = "{".to_owned();
        assert!(matches!(
            request(&broken).unwrap_err(),
            ClientError::Args(_)
        ));
    }
}
