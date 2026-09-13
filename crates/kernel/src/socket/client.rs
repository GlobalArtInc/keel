use std::io;
use std::path::{Path, PathBuf};

use keel_types::{Boot, Descriptor, Envelope};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use super::frame::Frame;
use crate::dispatcher::CallOptions;
use crate::http::Catalogue;

pub struct Client {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not connect to the control socket {path}: {source}")]
    Connect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "the control socket closed the connection without an answer; it admits the unix user the service runs as"
    )]
    Closed,
    #[error("the control socket connection failed: {0}")]
    Io(#[from] io::Error),
    #[error("the frame could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("the control socket answered with something that is not an envelope: {0}")]
    Decode(#[source] serde_json::Error),
}

impl Client {
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|source| ClientError::Connect {
                path: path.to_owned(),
                source,
            })?;
        let (read, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer,
        })
    }

    pub async fn call(
        &mut self,
        func: &str,
        params: Value,
        options: CallOptions,
    ) -> Result<Envelope<Value>, ClientError> {
        self.call_as(func, params, None, options).await
    }

    pub async fn call_as(
        &mut self,
        func: &str,
        params: Value,
        run_as: Option<&str>,
        options: CallOptions,
    ) -> Result<Envelope<Value>, ClientError> {
        self.send(&Frame::Call {
            func: func.to_owned(),
            params,
            request_id: options.request_id,
            idempotency_key: options.idempotency_key,
            run_as: run_as.map(str::to_owned),
        })
        .await
    }

    pub async fn describe(&mut self, name: &str) -> Result<Envelope<Descriptor>, ClientError> {
        self.send(&Frame::describe(name)).await
    }

    pub async fn i18n(&mut self, lang: &str) -> Result<Envelope<Catalogue>, ClientError> {
        self.send(&Frame::i18n(lang)).await
    }

    pub async fn boot(&mut self) -> Result<Envelope<Boot>, ClientError> {
        self.send(&Frame::Boot).await
    }

    pub async fn send<T: DeserializeOwned>(
        &mut self,
        frame: &Frame,
    ) -> Result<Envelope<T>, ClientError> {
        let line = frame.to_line().map_err(ClientError::Encode)?;
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;
        self.receive().await
    }

    pub async fn send_raw<T: DeserializeOwned>(
        &mut self,
        line: &str,
    ) -> Result<Envelope<T>, ClientError> {
        self.writer.write_all(line.as_bytes()).await?;
        if !line.ends_with('\n') {
            self.writer.write_all(b"\n").await?;
        }
        self.writer.flush().await?;
        self.receive().await
    }

    async fn receive<T: DeserializeOwned>(&mut self) -> Result<Envelope<T>, ClientError> {
        let mut answer = String::new();
        if self.reader.read_line(&mut answer).await? == 0 {
            return Err(ClientError::Closed);
        }
        serde_json::from_str(&answer).map_err(ClientError::Decode)
    }
}
