use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use keel_sdk::types::TaskState;
use keel_sdk::{Id, Result};

use super::runner::Runner;
use super::worker::{final_state, idle};

pub const WAIT_STEP: Duration = Duration::from_millis(50);

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
