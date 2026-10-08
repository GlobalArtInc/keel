use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use keel_sdk::serde_json;
use keel_sdk::system::{Frame, LiveEvents, ParamMasker, TaskRunner, TaskSpec};
use keel_sdk::types::{CallError, TaskProgress, TaskState};
use tokio::sync::Notify;

use super::entity::{job, step};
use super::log;
use super::sink::JobSink;
use super::worker::{STOP_GRACE, quiesce};

pub const FRAME_KIND: &str = "task.progress";
pub const ERROR_INTERRUPTED: &str = "task.error.interrupted";
pub const ERROR_NOT_RUNNING: &str = "task.error.not_running";
pub const ERROR_NOT_FOUND: &str = "task.error.not_found";
pub const ERROR_NOT_LOCAL: &str = "task.error.not_local";
pub const ERROR_FINISHED: &str = "task.error.finished";
pub const ERROR_STATE: &str = "task.error.state";

pub(super) struct Shared {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub live: Arc<dyn LiveEvents>,
    pub masker: Arc<dyn ParamMasker>,
    pub data_dir: PathBuf,
    pub wake: Notify,
    pub settled: Notify,
    draining: AtomicBool,
    running: Mutex<BTreeMap<Id, Arc<JobSink>>>,
}

#[derive(Default)]
pub struct Runner {
    shared: OnceLock<Arc<Shared>>,
}

impl Shared {
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    pub fn drain(&self) {
        self.draining.store(true, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn resume(&self) {
        self.draining.store(false, Ordering::SeqCst);
        self.wake.notify_one();
    }

    pub fn running(&self) -> Vec<(Id, Arc<JobSink>)> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(id, sink)| (*id, sink.clone()))
            .collect()
    }

    pub fn running_sink(&self, id: Id) -> Option<Arc<JobSink>> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    pub fn start_running(&self, id: Id, sink: Arc<JobSink>) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, sink);
    }

    pub fn stop_running(&self, id: Id) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        self.wake.notify_one();
        self.settled.notify_waiters();
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn publish(&self, row: &job::Model, state: TaskState) {
        let progress = TaskProgress {
            id: match row.id.parse() {
                Ok(id) => id,
                Err(_) => return,
            },
            function: row.function.clone(),
            state,
            done: u64::try_from(row.progress_done).unwrap_or_default(),
            total: row
                .progress_total
                .and_then(|total| u64::try_from(total).ok()),
            message: row.message.clone(),
        };
        self.publish_progress(row, progress);
    }

    pub fn publish_progress(&self, row: &job::Model, progress: TaskProgress) {
        let (Ok(principal), Ok(account)) = (row.principal_id.parse(), row.account_id.parse())
        else {
            return;
        };
        let data = match serde_json::to_value(&progress) {
            Ok(data) => data,
            Err(_) => return,
        };
        self.live.publish(Frame {
            principal,
            account,
            kind: FRAME_KIND.to_owned(),
            data,
        });
    }

    async fn insert(&self, db: &Db, id: Id, spec: TaskSpec) -> Result<job::Model> {
        let mut masked = spec.params.clone();
        self.masker.mask(&spec.function, &mut masked)?;
        let row = job::ActiveModel {
            id: Set(id.to_string()),
            class: Set(spec.class),
            function: Set(spec.function),
            params_json: Set(spec.params),
            params_masked_json: Set(masked),
            principal_id: Set(spec.principal.id.to_string()),
            principal_json: Set(serde_json::to_value(&spec.principal)?),
            account_id: Set(spec.principal.account_id.to_string()),
            state: Set(TaskState::Queued.as_str().to_owned()),
            resumable: Set(spec.resumable),
            progress_done: Set(0),
            progress_total: Set(None),
            message: Set(None),
            checkpoint_json: Set(None),
            result_json: Set(None),
            error_json: Set(None),
            request_id: Set(spec.request_id),
            created_at: Set(self.now()),
            started_at: Set(None),
            finished_at: Set(None),
            file_name: Set(None),
            file_type: Set(None),
        }
        .insert(db)
        .await?;
        Ok(row)
    }

    pub async fn load(&self, id: Id) -> Result<job::Model> {
        job::Entity::find_by_id(id.to_string())
            .one(&self.db)
            .await?
            .ok_or_else(|| KeelError::not_found(ERROR_NOT_FOUND).param("id", id.to_string()))
    }

    pub async fn last_seq(&self, id: Id) -> Result<i64> {
        let last = step::Entity::find()
            .filter(step::Column::JobId.eq(id.to_string()))
            .order_by_desc(step::Column::Seq)
            .limit(1)
            .one(&self.db)
            .await?;
        Ok(last.map(|row| row.seq).unwrap_or_default())
    }

    pub async fn record_step(
        &self,
        id: Id,
        seq: i64,
        done: u64,
        total: Option<u64>,
        message: Option<&str>,
        checkpoint: Option<Value>,
    ) -> Result<()> {
        let progress = checkpoint.is_none();
        step::ActiveModel {
            job_id: Set(id.to_string()),
            seq: Set(seq),
            at: Set(self.now()),
            done: Set(clamp(done)),
            total: Set(total.map(clamp)),
            message: Set(message.map(str::to_owned)),
            checkpoint_json: Set(checkpoint.clone()),
        }
        .insert(&self.db)
        .await?;
        let mut update = job::Entity::update_many().filter(job::Column::Id.eq(id.to_string()));
        if progress {
            update = update
                .col_expr(job::Column::ProgressDone, Expr::value(clamp(done)))
                .col_expr(job::Column::ProgressTotal, Expr::value(total.map(clamp)))
                .col_expr(
                    job::Column::Message,
                    Expr::value(message.map(str::to_owned)),
                );
        } else {
            update = update.col_expr(job::Column::CheckpointJson, Expr::value(checkpoint));
        }
        update.exec(&self.db).await?;
        Ok(())
    }

    pub async fn transition(
        &self,
        id: Id,
        from: &[TaskState],
        to: TaskState,
        outcome: Outcome,
    ) -> Result<Option<job::Model>> {
        let from: Vec<&str> = from.iter().map(|state| state.as_str()).collect();
        let now = self.now();
        let mut update = job::Entity::update_many()
            .filter(job::Column::Id.eq(id.to_string()))
            .filter(job::Column::State.is_in(from))
            .col_expr(job::Column::State, Expr::value(to.as_str()));
        update = match outcome {
            Outcome::Started => update.col_expr(job::Column::StartedAt, Expr::value(Some(now))),
            Outcome::Requeued => update
                .col_expr(job::Column::StartedAt, Expr::value(None::<DateTime<Utc>>))
                .col_expr(job::Column::FinishedAt, Expr::value(None::<DateTime<Utc>>))
                .col_expr(job::Column::ErrorJson, Expr::value(None::<Value>)),
            Outcome::Result { value, file } => update
                .col_expr(job::Column::FinishedAt, Expr::value(Some(now)))
                .col_expr(job::Column::ResultJson, Expr::value(Some(value)))
                .col_expr(
                    job::Column::FileName,
                    Expr::value(file.as_ref().map(|file| file.filename.clone())),
                )
                .col_expr(
                    job::Column::FileType,
                    Expr::value(file.map(|file| file.content_type)),
                ),
            Outcome::Error(error) => update
                .col_expr(job::Column::FinishedAt, Expr::value(Some(now)))
                .col_expr(
                    job::Column::ErrorJson,
                    Expr::value(Some(error_json(&error))),
                ),
            Outcome::Ended => update.col_expr(job::Column::FinishedAt, Expr::value(Some(now))),
        };
        if update.exec(&self.db).await?.rows_affected == 0 {
            return Ok(None);
        }
        let row = self.load(id).await?;
        self.publish(&row, to);
        Ok(Some(row))
    }
}

pub enum Outcome {
    Started,
    Requeued,
    Result {
        value: Value,
        file: Option<DownloadFile>,
    },
    Error(CallError),
    Ended,
}

pub fn interrupted() -> CallError {
    KeelError::unavailable(ERROR_INTERRUPTED).to_call_error()
}

fn error_json(error: &CallError) -> Value {
    serde_json::to_value(error).unwrap_or(Value::Null)
}

pub fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub fn state_of(row: &job::Model) -> Result<TaskState> {
    row.state.parse().map_err(|_| {
        KeelError::internal(ERROR_STATE)
            .param("id", row.id.as_str())
            .param("state", row.state.as_str())
    })
}

impl Runner {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn start(
        &self,
        db: Db,
        clock: Arc<dyn Clock>,
        live: Arc<dyn LiveEvents>,
        masker: Arc<dyn ParamMasker>,
        data_dir: &Path,
    ) -> Arc<Shared> {
        let shared = Arc::new(Shared {
            db,
            clock,
            live,
            masker,
            data_dir: data_dir.to_path_buf(),
            wake: Notify::new(),
            settled: Notify::new(),
            draining: AtomicBool::new(false),
            running: Mutex::new(BTreeMap::new()),
        });
        if self.shared.set(shared.clone()).is_err() {
            tracing::warn!("the task runner was started twice; the first start stays");
        }
        shared
    }

    pub(super) fn shared(&self) -> Result<&Arc<Shared>> {
        self.shared
            .get()
            .ok_or_else(|| KeelError::unavailable(ERROR_NOT_RUNNING))
    }

    pub fn is_started(&self) -> bool {
        self.shared.get().is_some()
    }
}

#[async_trait]
impl TaskRunner for Runner {
    async fn spawn(&self, spec: TaskSpec) -> Result<Id> {
        let shared = self.shared()?;
        let id = Id::generate();
        let row = shared.insert(&shared.db, id, spec).await?;
        shared.publish(&row, TaskState::Queued);
        shared.wake.notify_one();
        Ok(id)
    }

    async fn enqueue(&self, db: &Db, spec: TaskSpec) -> Result<Id> {
        let shared = self.shared()?;
        let id = Id::generate();
        shared.insert(db, id, spec).await?;
        Ok(id)
    }

    async fn wake(&self, id: Id) -> Result<()> {
        let shared = self.shared()?;
        let published = async {
            let row = shared.load(id).await?;
            shared.publish(&row, state_of(&row)?);
            Ok(())
        }
        .await;
        shared.wake.notify_one();
        published
    }

    async fn progress(
        &self,
        id: Id,
        done: u64,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<()> {
        let shared = self.shared()?;
        if let Some(sink) = shared.running_sink(id) {
            return sink.report(done, total, message).await;
        }
        let seq = shared.last_seq(id).await? + 1;
        shared
            .record_step(id, seq, done, total, message, None)
            .await?;
        let row = shared.load(id).await?;
        shared.publish(&row, state_of(&row)?);
        Ok(())
    }

    async fn log(&self, id: Id, line: &str) -> Result<()> {
        let shared = self.shared()?;
        log::append(&shared.data_dir, id, shared.now(), line).await
    }

    async fn checkpoint(&self, id: Id, state: Value) -> Result<()> {
        let shared = self.shared()?;
        if let Some(sink) = shared.running_sink(id) {
            return sink.save(state).await;
        }
        let row = shared.load(id).await?;
        let seq = shared.last_seq(id).await? + 1;
        shared
            .record_step(
                id,
                seq,
                u64::try_from(row.progress_done).unwrap_or_default(),
                row.progress_total
                    .and_then(|total| u64::try_from(total).ok()),
                row.message.as_deref(),
                Some(state),
            )
            .await
    }

    async fn cancel(&self, id: Id) -> Result<()> {
        let shared = self.shared()?;
        let row = shared.load(id).await?;
        match state_of(&row)? {
            TaskState::Running if let Some(sink) = shared.running_sink(id) => {
                sink.request_cancel();
                Ok(())
            }
            TaskState::Queued => {
                shared
                    .transition(
                        id,
                        &[TaskState::Queued],
                        TaskState::Cancelled,
                        Outcome::Ended,
                    )
                    .await?;
                Ok(())
            }
            TaskState::Running => Err(KeelError::conflict(ERROR_NOT_LOCAL).param("id", row.id)),
            state => Err(KeelError::conflict(ERROR_FINISHED)
                .param("id", row.id)
                .param("state", state.as_str())),
        }
    }

    async fn state(&self, id: Id) -> Result<TaskState> {
        let shared = self.shared()?;
        state_of(&shared.load(id).await?)
    }

    async fn quiesce(&self, except: Id) -> Result<Vec<Id>> {
        let shared = self.shared()?;
        Ok(quiesce(shared, except, STOP_GRACE).await)
    }

    fn resume(&self) -> Result<()> {
        self.shared()?.resume();
        Ok(())
    }
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("started", &self.is_started())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::types::ErrorCode;

    use super::*;

    #[test]
    fn progress_values_are_clamped_into_the_bigint_columns() {
        assert_eq!(clamp(0), 0);
        assert_eq!(clamp(42), 42);
        assert_eq!(clamp(u64::MAX), i64::MAX);
    }

    #[test]
    fn an_unknown_stored_state_is_an_internal_error_naming_the_row() {
        let row = job::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".into(),
            class: "default".into(),
            function: "note.export".into(),
            params_json: json!({}),
            params_masked_json: json!({}),
            principal_id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".into(),
            principal_json: json!({}),
            account_id: "01J8A0000000000000000000A1".into(),
            state: "paused".into(),
            resumable: false,
            progress_done: 0,
            progress_total: None,
            message: None,
            checkpoint_json: None,
            result_json: None,
            error_json: None,
            request_id: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            file_name: None,
            file_type: None,
        };
        let error = state_of(&row).expect_err("paused is not a state");
        assert_eq!(error.key(), ERROR_STATE);
        assert_eq!(error.params()["state"], json!("paused"));
        let interrupted = interrupted();
        assert_eq!(interrupted.key, ERROR_INTERRUPTED);
        assert_eq!(interrupted.code, ErrorCode::Unavailable);
        assert!(!Runner::new().is_started());
        assert_eq!(format!("{:?}", Runner::new()), "Runner { started: false }");
    }
}
