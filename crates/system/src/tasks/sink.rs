use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Mutex, PoisonError};

use keel_sdk::prelude::*;
use keel_sdk::system::TaskSink;
use keel_sdk::types::{TaskProgress, TaskState};

use super::entity::job;
use super::log;
use super::runner::Shared;

pub struct JobSink {
    shared: Arc<Shared>,
    row: job::Model,
    id: Id,
    cancelled: AtomicBool,
    seq: AtomicI64,
    last: Mutex<Reported>,
    file: Mutex<Option<DownloadFile>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Reported {
    done: u64,
    total: Option<u64>,
    message: Option<String>,
}

impl JobSink {
    pub(super) fn new(shared: Arc<Shared>, row: job::Model, id: Id, last_seq: i64) -> Self {
        let last = Reported {
            done: u64::try_from(row.progress_done).unwrap_or_default(),
            total: row
                .progress_total
                .and_then(|total| u64::try_from(total).ok()),
            message: row.message.clone(),
        };
        Self {
            shared,
            row,
            id,
            cancelled: AtomicBool::new(false),
            seq: AtomicI64::new(last_seq),
            last: Mutex::new(last),
            file: Mutex::new(None),
        }
    }

    pub fn stored_file(&self) -> Option<DownloadFile> {
        self.file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn cancel_requested(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn function(&self) -> &str {
        &self.row.function
    }

    pub fn class(&self) -> &str {
        &self.row.class
    }

    pub(super) fn row(&self) -> &job::Model {
        &self.row
    }

    fn next_seq(&self) -> i64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn last(&self) -> Reported {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) async fn report(
        &self,
        done: u64,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<()> {
        let seq = self.next_seq();
        self.shared
            .record_step(self.id, seq, done, total, message, None)
            .await?;
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Reported {
            done,
            total,
            message: message.map(str::to_owned),
        };
        self.shared.publish_progress(
            &self.row,
            TaskProgress {
                id: self.id,
                function: self.row.function.clone(),
                state: TaskState::Running,
                done,
                total,
                message: message.map(str::to_owned),
            },
        );
        Ok(())
    }

    pub(super) async fn save(&self, state: Value) -> Result<()> {
        let seq = self.next_seq();
        let last = self.last();
        self.shared
            .record_step(
                self.id,
                seq,
                last.done,
                last.total,
                last.message.as_deref(),
                Some(state),
            )
            .await
    }
}

#[async_trait]
impl TaskSink for JobSink {
    fn id(&self) -> Id {
        self.id
    }

    async fn progress(&self, done: u64, total: Option<u64>, message: Option<&str>) -> Result<()> {
        self.report(done, total, message).await
    }

    async fn log(&self, line: &str) -> Result<()> {
        log::append(&self.shared.data_dir, self.id, self.shared.now(), line).await
    }

    async fn checkpoint(&self, state: Value) -> Result<()> {
        self.save(state).await
    }

    fn resume_from(&self) -> Option<Value> {
        self.row.checkpoint_json.clone()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_requested() || self.shared.is_draining()
    }

    fn output_path(&self) -> PathBuf {
        log::output_path(&self.shared.data_dir, self.id)
    }

    fn stored(&self, file: DownloadFile) {
        *self.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(file);
    }
}
