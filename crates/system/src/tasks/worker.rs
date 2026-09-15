use std::collections::BTreeSet;
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_sdk::serde_json;
use keel_sdk::system::{AdvisoryLock, Caller, CallerOptions};
use keel_sdk::task::CANCELLED_KEY;
use keel_sdk::types::{ErrorCode, TaskState};
use keel_sdk::{Context, TaskHandle};
use tokio::time::Instant;

use super::entity::job;
use super::leader;
use super::runner::{Outcome, Shared, interrupted, state_of};
use super::sink::JobSink;

pub const POLL: Duration = Duration::from_secs(1);
pub const STOP_GRACE: Duration = Duration::from_secs(20);
pub const CALLER_UNAVAILABLE: &str = "keel.error.caller_unavailable";

pub(super) struct Worker {
    shared: Arc<Shared>,
    ctx: Context,
    caller: Arc<dyn Caller>,
    lock: Arc<dyn AdvisoryLock>,
}

impl Worker {
    pub fn new(
        shared: Arc<Shared>,
        ctx: Context,
        caller: Arc<dyn Caller>,
        lock: Arc<dyn AdvisoryLock>,
    ) -> Self {
        Self {
            shared,
            ctx,
            caller,
            lock,
        }
    }

    pub async fn run(self) {
        let _guard = match leader::hold(self.lock.as_ref()).await {
            Ok(guard) => guard,
            Err(error) => {
                tracing::error!(error = %error, "the task worker could not take its lock");
                return;
            }
        };
        tracing::info!("the task worker holds the lock");
        if let Err(error) = recover(&self.shared).await {
            tracing::error!(error = %error, "the task worker could not recover the queue");
        }
        loop {
            if !self.shared.is_draining()
                && let Err(error) = self.schedule().await
            {
                tracing::error!(error = %error, "the task worker could not read the queue");
            }
            tokio::select! {
                _ = self.shared.wake.notified() => {}
                _ = tokio::time::sleep(POLL) => {}
            }
        }
    }

    async fn schedule(&self) -> Result<()> {
        let mut busy: BTreeSet<String> = self
            .shared
            .running()
            .into_iter()
            .map(|(_, sink)| sink.class().to_owned())
            .collect();
        let queued = job::Entity::find()
            .filter(job::Column::State.eq(TaskState::Queued.as_str()))
            .order_by_asc(job::Column::CreatedAt)
            .order_by_asc(job::Column::Id)
            .all(&self.shared.db)
            .await?;
        for row in queued {
            if busy.contains(&row.class) {
                continue;
            }
            let Ok(id) = row.id.parse::<Id>() else {
                continue;
            };
            let Some(row) = self
                .shared
                .transition(
                    id,
                    &[TaskState::Queued],
                    TaskState::Running,
                    Outcome::Started,
                )
                .await?
            else {
                continue;
            };
            busy.insert(row.class.clone());
            let last_seq = self.shared.last_seq(id).await?;
            let sink = Arc::new(JobSink::new(self.shared.clone(), row, id, last_seq));
            self.shared.start_running(id, sink.clone());
            let job = Job {
                shared: self.shared.clone(),
                caller: self.caller.clone(),
                sink,
                id,
            };
            self.ctx.spawn(job.run());
        }
        Ok(())
    }
}

struct Job {
    shared: Arc<Shared>,
    caller: Arc<dyn Caller>,
    sink: Arc<JobSink>,
    id: Id,
}

impl Job {
    async fn run(self) {
        let outcome = self.perform().await;
        if let Err(error) = self.finish(outcome).await {
            tracing::error!(task = %self.id, error = %error, "the job outcome could not be stored");
        }
        self.shared.stop_running(self.id);
    }

    async fn perform(&self) -> Result<Value> {
        let principal: Principal = serde_json::from_value(self.sink.row().principal_json.clone())
            .map_err(|error| {
            KeelError::internal("task.error.principal")
                .param("id", self.id.to_string())
                .source(error)
        })?;
        self.caller
            .call(
                principal,
                self.sink.function(),
                self.sink.row().params_json.clone(),
                CallerOptions {
                    request_id: Some(format!("task-{}", self.id.to_string().to_lowercase())),
                    task: Some(TaskHandle::new(self.sink.clone())),
                },
            )
            .await
    }

    async fn finish(&self, outcome: Result<Value>) -> Result<()> {
        let cancelled = self.sink.cancel_requested();
        let draining = self.shared.is_draining();
        let (state, stored) = match outcome {
            Ok(value) if cancelled => (TaskState::Cancelled, Outcome::Result(value)),
            Ok(value) => (TaskState::Done, Outcome::Result(value)),
            Err(error) if error.key() == CALLER_UNAVAILABLE => {
                tracing::warn!(task = %self.id, "the caller is not filled yet; the job is queued again");
                (TaskState::Queued, Outcome::Requeued)
            }
            Err(error) if error.key() == CANCELLED_KEY && cancelled => {
                (TaskState::Cancelled, Outcome::Ended)
            }
            Err(error) if error.key() == CANCELLED_KEY && draining => {
                (TaskState::Interrupted, Outcome::Error(interrupted()))
            }
            Err(error) if error.key() == CANCELLED_KEY => (TaskState::Cancelled, Outcome::Ended),
            Err(error) => {
                if error.code() == ErrorCode::Internal {
                    tracing::error!(task = %self.id, error = %error, "the job failed");
                }
                (TaskState::Failed, Outcome::Error(error.to_call_error()))
            }
        };
        self.shared
            .transition(self.id, &[TaskState::Running], state, stored)
            .await?;
        Ok(())
    }
}

pub(super) async fn recover(shared: &Shared) -> Result<()> {
    let left_behind = job::Entity::find()
        .filter(
            job::Column::State
                .is_in([TaskState::Running.as_str(), TaskState::Interrupted.as_str()]),
        )
        .all(&shared.db)
        .await?;
    for row in left_behind {
        let Ok(id) = row.id.parse::<Id>() else {
            continue;
        };
        let (to, outcome) = if row.resumable {
            (TaskState::Queued, Outcome::Requeued)
        } else if row.state == TaskState::Running.as_str() {
            (TaskState::Interrupted, Outcome::Error(interrupted()))
        } else {
            continue;
        };
        shared
            .transition(
                id,
                &[TaskState::Running, TaskState::Interrupted],
                to,
                outcome,
            )
            .await?;
    }
    Ok(())
}

pub(super) async fn drain(shared: &Shared, grace: Duration) -> Result<()> {
    shared.drain();
    let deadline = Instant::now() + grace;
    loop {
        let settled = shared.settled.notified();
        if shared.running().is_empty() {
            break;
        }
        if tokio::time::timeout_at(deadline, settled).await.is_err() {
            break;
        }
    }
    for (id, _) in shared.running() {
        shared
            .transition(
                id,
                &[TaskState::Running],
                TaskState::Interrupted,
                Outcome::Error(interrupted()),
            )
            .await?;
    }
    Ok(())
}

pub(super) async fn idle(shared: &Shared) -> Result<bool> {
    if !shared.running().is_empty() {
        return Ok(false);
    }
    let open = job::Entity::find()
        .filter(job::Column::State.is_in([TaskState::Queued.as_str(), TaskState::Running.as_str()]))
        .all(&shared.db)
        .await?;
    Ok(open.is_empty())
}

pub(super) async fn final_state(shared: &Shared, id: Id) -> Result<Option<TaskState>> {
    let row = shared.load(id).await?;
    let state = state_of(&row)?;
    Ok(state.is_final().then_some(state))
}
