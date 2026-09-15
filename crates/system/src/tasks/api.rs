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

    pub async fn wait_until(&self, id: Id, state: TaskState) -> Result<()> {
        let shared = self.runner.shared()?;
        loop {
            let row = shared.load(id).await?;
            let current = super::runner::state_of(&row)?;
            if current == state {
                return Ok(());
            }
            if current.is_final() {
                return Err(keel_sdk::KeelError::conflict(super::runner::ERROR_FINISHED)
                    .param("id", id.to_string())
                    .param("state", current.as_str()));
            }
            tokio::time::sleep(WAIT_STEP).await;
        }
    }
}
