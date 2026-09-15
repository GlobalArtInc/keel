use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use keel_types::Id;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{KeelError, Result};
use crate::system::TaskSink;

pub const CANCELLED_KEY: &str = "task.error.cancelled";
pub const OUTPUT_EXTENSION: &str = "out";

#[derive(Clone)]
pub struct TaskHandle(Arc<dyn TaskSink>);

impl TaskHandle {
    #[doc(hidden)]
    pub fn new(sink: Arc<dyn TaskSink>) -> Self {
        Self(sink)
    }

    pub fn id(&self) -> Id {
        self.0.id()
    }

    pub async fn progress(
        &self,
        done: u64,
        total: Option<u64>,
        message: Option<&str>,
    ) -> Result<()> {
        self.0.progress(done, total, message).await
    }

    pub async fn log(&self, line: &str) -> Result<()> {
        self.0.log(line).await
    }

    pub async fn checkpoint<T: Serialize + Sync>(&self, state: &T) -> Result<()> {
        self.0.checkpoint(serde_json::to_value(state)?).await
    }

    pub fn resume_from<T: DeserializeOwned>(&self) -> Result<Option<T>> {
        self.0
            .resume_from()
            .map(|state| {
                serde_json::from_value(state).map_err(|error| {
                    KeelError::internal("keel.error.task_checkpoint")
                        .param("task", self.id().to_string())
                        .source(error)
                })
            })
            .transpose()
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    pub fn cancelled(&self) -> KeelError {
        cancelled(self.id())
    }

    pub fn output(&self) -> Result<TaskOutput> {
        TaskOutput::open(self.0.output_path())
    }
}

pub fn cancelled(id: Id) -> KeelError {
    KeelError::unavailable(CANCELLED_KEY).param("task", id.to_string())
}

impl fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskHandle")
            .field("id", &self.id())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

pub struct TaskOutput {
    path: PathBuf,
    file: File,
}

impl TaskOutput {
    fn open(path: PathBuf) -> Result<Self> {
        let failed = |source: io::Error| {
            KeelError::internal("keel.error.task_output")
                .param("path", path.display().to_string())
                .source(source)
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(failed)?;
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Write for TaskOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl fmt::Debug for TaskOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskOutput")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::*;

    struct Recording {
        id: Id,
        dir: PathBuf,
        cancelled: bool,
        checkpoint: Option<Value>,
        seen: Mutex<Vec<String>>,
    }

    impl Default for Recording {
        fn default() -> Self {
            Self {
                id: Id::generate(),
                dir: PathBuf::new(),
                cancelled: false,
                checkpoint: None,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl TaskSink for Recording {
        fn id(&self) -> Id {
            self.id
        }

        async fn progress(
            &self,
            done: u64,
            total: Option<u64>,
            message: Option<&str>,
        ) -> Result<()> {
            self.seen
                .lock()
                .unwrap()
                .push(format!("progress {done} {total:?} {message:?}"));
            Ok(())
        }

        async fn log(&self, line: &str) -> Result<()> {
            self.seen.lock().unwrap().push(format!("log {line}"));
            Ok(())
        }

        async fn checkpoint(&self, state: Value) -> Result<()> {
            self.seen
                .lock()
                .unwrap()
                .push(format!("checkpoint {state}"));
            Ok(())
        }

        fn resume_from(&self) -> Option<Value> {
            self.checkpoint.clone()
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled
        }

        fn output_path(&self) -> PathBuf {
            self.dir.join(format!("{}.{OUTPUT_EXTENSION}", self.id))
        }
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct Cursor {
        last: u32,
    }

    #[tokio::test]
    async fn the_handle_forwards_reports_and_reads_the_checkpoint_back_typed() {
        let dir = std::env::temp_dir().join(format!("keel-task-handle-{}", Id::generate()));
        let sink = Arc::new(Recording {
            id: Id::generate(),
            dir: dir.clone(),
            cancelled: true,
            checkpoint: Some(json!({ "last": 4 })),
            seen: Mutex::new(Vec::new()),
        });
        let handle = TaskHandle::new(sink.clone());
        assert_eq!(handle.id(), sink.id);
        handle.progress(1, Some(3), Some("first")).await.unwrap();
        handle.log("line").await.unwrap();
        handle.checkpoint(&Cursor { last: 1 }).await.unwrap();
        assert_eq!(
            *sink.seen.lock().unwrap(),
            [
                "progress 1 Some(3) Some(\"first\")",
                "log line",
                "checkpoint {\"last\":1}"
            ]
        );
        assert_eq!(
            handle.resume_from::<Cursor>().unwrap(),
            Some(Cursor { last: 4 })
        );
        assert!(handle.is_cancelled());
        let error = handle.cancelled();
        assert_eq!(error.key(), CANCELLED_KEY);
        assert_eq!(error.params()["task"], json!(sink.id.to_string()));
        assert!(format!("{handle:?}").contains("cancelled: true"));

        let mut output = handle.output().unwrap();
        output.write_all(b"a,b\n").unwrap();
        output.flush().unwrap();
        drop(output);
        let mut again = handle.output().unwrap();
        again.write_all(b"c,d\n").unwrap();
        drop(again);
        let written = std::fs::read_to_string(dir.join(format!("{}.out", sink.id))).unwrap();
        assert_eq!(written, "a,b\nc,d\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_checkpoint_of_another_shape_is_an_internal_error_and_none_stays_none() {
        let sink = Arc::new(Recording {
            checkpoint: Some(json!({ "other": true })),
            ..Recording::default()
        });
        let handle = TaskHandle::new(sink);
        let error = handle.resume_from::<Cursor>().unwrap_err();
        assert_eq!(error.key(), "keel.error.task_checkpoint");
        let fresh = TaskHandle::new(Arc::new(Recording::default()));
        assert_eq!(fresh.resume_from::<Cursor>().unwrap(), None);
        assert!(!fresh.is_cancelled());
    }
}
