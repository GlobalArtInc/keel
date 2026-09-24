use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use keel_host_agent::{Agent, Failure, Operation, OperationCall, Policy, Template};
use keel_sdk::registrar::Contributions;
use keel_sdk::{AgentOp, AgentOpError, AgentScope, ConfigTemplate};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::harness::Error;

pub(crate) const SECTION: &str = "host";
const POLICY_FILE: &str = "agent.yaml";
const SOCKET_FILE: &str = "agent.sock";
const TEMPLATES_DIR: &str = "templates";

pub(crate) struct Prepared {
    dir: TempDir,
    policy: Policy,
    uid: u32,
}

pub(crate) struct InProcessAgent {
    dir: TempDir,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Prepared {
    pub(crate) fn new(policy: &str) -> Result<Self, Error> {
        let dir = tempfile::tempdir().map_err(Error::AgentDirectory)?;
        let path = dir.path().join(POLICY_FILE);
        fs::write(&path, policy).map_err(Error::AgentDirectory)?;
        let policy = Policy::load(&path)?;
        let uid = fs::metadata(dir.path())
            .map_err(Error::AgentDirectory)?
            .uid();
        Ok(Self { dir, policy, uid })
    }

    pub(crate) fn socket(&self) -> PathBuf {
        self.dir.path().join(SOCKET_FILE)
    }

    pub(crate) async fn start(
        self,
        contributions: &Contributions,
    ) -> Result<InProcessAgent, Error> {
        let socket = self.socket();
        let operations = contributions
            .of::<AgentOp>()
            .cloned()
            .map(operation)
            .collect();
        let templates = contributions.of::<ConfigTemplate>().map(template).collect();
        let bound = Agent::new(self.policy, self.uid, self.dir.path().join(TEMPLATES_DIR))
            .extend(operations, templates)?
            .bind(&socket)
            .await?;
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(bound.serve(async move {
            stopped.await.ok();
        }));
        Ok(InProcessAgent {
            dir: self.dir,
            stop,
            task,
        })
    }
}

impl InProcessAgent {
    pub(crate) async fn stop(self) {
        let InProcessAgent { dir, stop, task } = self;
        if stop.send(()).is_err() {
            tracing::warn!("the in-process agent had stopped before the harness");
        }
        if let Err(error) = task.await {
            tracing::warn!(error = %error, "the in-process agent did not stop cleanly");
        }
        drop(dir);
    }
}

struct Scope<'a, 'b>(&'a OperationCall<'b>);

impl AgentScope for Scope<'_, '_> {
    fn product(&self) -> &str {
        self.0.product()
    }

    fn policy(&self) -> &Value {
        self.0.policy()
    }

    fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
        self.0
            .resolve(root, name)
            .map_err(|failure| AgentOpError::new(failure.key(), failure.detail()))
    }
}

fn operation(op: AgentOp) -> Operation {
    let name = op.name().to_owned();
    let class = op.class().to_owned();
    Operation::new(&name, &class, move |call, args| {
        op.run(&Scope(call), args)
            .map_err(|error| Failure::new(error.key(), error.detail()))
    })
}

fn template(template: &ConfigTemplate) -> Template {
    Template::new(template.id(), template.text(), template.check())
}
