use std::path::PathBuf;

use keel_kernel::ctl::{self, CtlError, Out, Request};
use keel_kernel::socket::{Client, ClientError, Serving, Socket, SocketError};
use keel_kernel::{CallOptions, Dispatcher};
use keel_types::{Boot, Envelope};
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;

use crate::harness::Harness;

pub struct TestSocket {
    path: PathBuf,
    serving: Option<Serving>,
    dir: Option<TempDir>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CtlOutput {
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SocketTestError {
    #[error("no temporary directory for the control socket: {0}")]
    Dir(#[source] std::io::Error),
    #[error(transparent)]
    Socket(#[from] SocketError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Ctl(#[from] CtlError),
}

impl Harness {
    pub async fn socket(&self) -> TestSocket {
        match TestSocket::try_serve(self.dispatcher().clone()).await {
            Ok(socket) => socket,
            Err(error) => panic!("{error}"),
        }
    }
}

impl TestSocket {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            serving: None,
            dir: None,
        }
    }

    async fn try_serve(dispatcher: Arc<Dispatcher>) -> Result<Self, SocketTestError> {
        let dir = tempfile::tempdir().map_err(SocketTestError::Dir)?;
        let path = dir.path().join("core.sock");
        let serving = Socket::new(dispatcher).start(&path).await?;
        Ok(Self {
            path,
            serving: Some(serving),
            dir: Some(dir),
        })
    }

    pub async fn call(&self, func: &str, params: Value) -> Envelope<Value> {
        match self.try_call(func, params).await {
            Ok(envelope) => envelope,
            Err(error) => panic!("{func} over the control socket: {error}"),
        }
    }

    async fn try_call(
        &self,
        func: &str,
        params: Value,
    ) -> Result<Envelope<Value>, SocketTestError> {
        let mut client = Client::connect(&self.path).await?;
        Ok(client.call(func, params, CallOptions::default()).await?)
    }

    pub async fn boot(&self) -> Envelope<Boot> {
        match self.try_boot().await {
            Ok(envelope) => envelope,
            Err(error) => panic!("boot over the control socket: {error}"),
        }
    }

    async fn try_boot(&self) -> Result<Envelope<Boot>, SocketTestError> {
        let mut client = Client::connect(&self.path).await?;
        Ok(client.boot().await?)
    }

    pub async fn ctl_json(&self, func: &str, pairs: &[&str], lang: &str) -> CtlOutput {
        self.ctl(func, pairs, Out::Json, lang).await
    }

    pub async fn ctl_table(&self, func: &str, pairs: &[&str], lang: &str) -> CtlOutput {
        self.ctl(func, pairs, Out::Table, lang).await
    }

    async fn ctl(&self, func: &str, pairs: &[&str], out: Out, lang: &str) -> CtlOutput {
        match self.try_ctl(func, pairs, out, lang).await {
            Ok(output) => output,
            Err(error) => panic!("ctl {func} over the control socket: {error}"),
        }
    }

    async fn try_ctl(
        &self,
        func: &str,
        pairs: &[&str],
        out: Out,
        lang: &str,
    ) -> Result<CtlOutput, SocketTestError> {
        let request = Request {
            func: func.to_owned(),
            params: ctl::params(pairs.iter().copied())?,
            out,
            lang: lang.to_owned(),
        };
        let output = ctl::run(&self.path, &request).await?;
        Ok(CtlOutput {
            ok: output.ok,
            text: output.text,
        })
    }

    pub async fn stop(mut self) -> Result<(), SocketTestError> {
        if let Some(serving) = self.serving.take() {
            serving.stop().await?;
        }
        drop(self.dir.take());
        Ok(())
    }
}

impl std::fmt::Debug for TestSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestSocket")
            .field("path", &self.path)
            .field("serving", &self.serving.is_some())
            .finish()
    }
}
