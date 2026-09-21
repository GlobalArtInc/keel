mod channel;
mod request;
mod update;

use std::future::Future;
use std::path::Path;

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
}

pub(crate) async fn serve(socket: &Path, policy: &Path, shutdown: impl Future<Output = ()>) {
    println!(
        "keel privileged: serving the update policy alone; the host policy arrives with the host module (socket {}, policy {})",
        socket.display(),
        policy.display()
    );
    shutdown.await;
    println!("keel privileged: stopped");
}
