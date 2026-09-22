mod channel;
mod request;
mod update;

use std::future::Future;
use std::path::Path;

use keel_host_agent::journal::JournalError;
use keel_host_agent::{Agent, AgentError, Policy, PolicyError};
use keel_release::ReleaseError;

pub(crate) use channel::{Source, SourceError};
pub(crate) use request::{Request, RequestError};
pub(crate) use update::{Runner, UpdateError};

pub(crate) const DEFAULT_SOCKET: &str = "/run/keel/agent.sock";
pub(crate) const DEFAULT_POLICY: &str = "/etc/keel/agent.yaml";
pub(crate) const DEFAULT_REQUEST: &str = "/var/lib/keel/update.request";
pub(crate) const DEFAULT_OPT: &str = "/opt/keel";
pub(crate) const DEFAULT_UNIT_DIR: &str = "/etc/systemd/system";
pub(crate) const DEFAULT_SYSTEMCTL: &str = "systemctl";

#[derive(Debug, thiserror::Error)]
pub(crate) enum PrivilegedError {
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error(transparent)]
    Update(#[from] UpdateError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Journal(#[from] JournalError),
}

pub(crate) struct HostPolicy<'a> {
    pub(crate) socket: &'a Path,
    pub(crate) policy: &'a Path,
    pub(crate) user: &'a str,
    pub(crate) templates: &'a Path,
}

pub(crate) async fn serve(
    host: HostPolicy<'_>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), PrivilegedError> {
    keel_host_agent::journal::init()?;
    let policy = Policy::load(host.policy)?;
    let agent = Agent::for_user(policy, host.user, host.templates.to_owned())?;
    tracing::info!(
        policy = %host.policy.display(),
        user = %host.user,
        admitted_uid = agent.admitted(),
        "keel privileged serves the host policy; the update policy runs from the request file"
    );
    let bound = agent.bind(host.socket).await?;
    bound.serve(shutdown).await;
    Ok(())
}
