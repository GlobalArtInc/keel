use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use keel_sdk::types::TaskState;
use keel_sdk::{Id, Result};

use super::runner::Runner;
use super::worker::{STOP_GRACE, final_state, idle, quiesce};

pub const WAIT_STEP: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quiesced {
    pub still_running: Vec<Id>,
}

pub struct Drain {
    runner: Arc<Runner>,
}

impl Drain {
    pub(super) fn new(runner: Arc<Runner>) -> Self {
        Self { runner }
    }

    pub fn data_dir(&self) -> Result<PathBuf> {
        Ok(self.runner.shared()?.data_dir.clone())
    }

    pub async fn drain(&self) -> Result<()> {
        let shared = self.runner.shared()?;
        loop {
            let settled = shared.settled.notified();
            if idle(shared).await? {
                return Ok(());
            }
            let _ = tokio::time::timeout(WAIT_STEP, settled).await;
        }
    }

    pub async fn quiesce(&self, except: Id) -> Result<Quiesced> {
        let shared = self.runner.shared()?;
        let still_running = quiesce(shared, except, STOP_GRACE).await;
        Ok(Quiesced { still_running })
    }

    pub fn resume(&self) -> Result<()> {
        self.runner.shared()?.resume();
        Ok(())
    }

    pub fn is_quiesced(&self) -> Result<bool> {
        Ok(self.runner.shared()?.is_draining())
    }

    pub async fn wait(&self, id: Id) -> Result<TaskState> {
        let shared = self.runner.shared()?;
        loop {
            let settled = shared.settled.notified();
            if let Some(state) = final_state(shared, id).await? {
                return Ok(state);
            }
            let _ = tokio::time::timeout(WAIT_STEP, settled).await;
        }
    }
}
