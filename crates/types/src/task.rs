use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::id::Id;

pub const DOWNLOAD_FUNCTION: &str = "task.download";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, thiserror::Error)]
#[error("unknown task state: {0}")]
pub struct UnknownTaskState(String);

impl TaskState {
    pub const ALL: [TaskState; 6] = [
        TaskState::Queued,
        TaskState::Running,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Interrupted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Running => "running",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
            TaskState::Interrupted => "interrupted",
        }
    }

    pub const fn is_final(self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed | TaskState::Cancelled | TaskState::Interrupted
        )
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TaskState {
    type Err = UnknownTaskState;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        TaskState::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
            .ok_or_else(|| UnknownTaskState(text.to_owned()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRef {
    pub task: TaskLink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskLink {
    pub id: Id,
}

impl TaskRef {
    pub fn new(id: Id) -> Self {
        Self {
            task: TaskLink { id },
        }
    }

    pub fn id(&self) -> Id {
        self.task.id
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFile {
    pub filename: String,
    pub content_type: String,
}

impl DownloadFile {
    pub fn new(filename: impl Into<String>, content_type: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadRef {
    pub download: DownloadLink,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadLink {
    pub function: String,
    pub params: Map<String, Value>,
}

impl DownloadRef {
    pub fn new(function: impl Into<String>, params: Map<String, Value>) -> Self {
        Self {
            download: DownloadLink {
                function: function.into(),
                params,
            },
        }
    }

    pub fn of_task(id: Id) -> Self {
        let mut params = Map::new();
        params.insert("id".to_owned(), Value::String(id.to_string()));
        Self::new(DOWNLOAD_FUNCTION, params)
    }

    pub fn function(&self) -> &str {
        &self.download.function
    }

    pub fn params(&self) -> &Map<String, Value> {
        &self.download.params
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GroupResult {
    pub updated: u64,
}

impl GroupResult {
    pub fn new(updated: u64) -> Self {
        Self { updated }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub id: Id,
    pub function: String,
    pub state: TaskState,
    pub done: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_state_names_itself_and_parses_back() {
        for state in TaskState::ALL {
            assert_eq!(state.as_str().parse::<TaskState>().unwrap(), state);
            assert_eq!(state.to_string(), state.as_str());
            assert_eq!(serde_json::to_value(state).unwrap(), json!(state.as_str()));
        }
        assert_eq!(
            "paused".parse::<TaskState>().unwrap_err().to_string(),
            "unknown task state: paused"
        );
    }

    #[test]
    fn queued_and_running_are_the_only_open_states() {
        let open: Vec<TaskState> = TaskState::ALL
            .into_iter()
            .filter(|state| !state.is_final())
            .collect();
        assert_eq!(open, [TaskState::Queued, TaskState::Running]);
    }

    #[test]
    fn a_task_ref_answers_the_job_under_task() {
        let id: Id = "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().unwrap();
        let reference = TaskRef::new(id);
        assert_eq!(reference.id(), id);
        assert_eq!(
            serde_json::to_value(reference).unwrap(),
            json!({ "task": { "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" } })
        );
        let parsed: TaskRef =
            serde_json::from_value(json!({ "task": { "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" } }))
                .unwrap();
        assert_eq!(parsed, reference);
    }

    #[test]
    fn a_download_ref_points_at_task_download_with_the_job_id_and_a_file_names_its_type() {
        let id: Id = "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().unwrap();
        let reference = DownloadRef::of_task(id);
        assert_eq!(reference.function(), DOWNLOAD_FUNCTION);
        assert_eq!(
            serde_json::to_value(&reference).unwrap(),
            json!({ "download": { "function": "task.download", "params": { "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3" } } })
        );
        let parsed: DownloadRef = serde_json::from_value(json!({
            "download": { "function": "dns.zone.export", "params": { "zone": "example.org" } }
        }))
        .unwrap();
        assert_eq!(parsed.function(), "dns.zone.export");
        assert_eq!(parsed.params()["zone"], json!("example.org"));
        let file = DownloadFile::new("notes.csv", "text/csv; charset=utf-8");
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            json!({ "filename": "notes.csv", "contentType": "text/csv; charset=utf-8" })
        );
    }

    #[test]
    fn a_group_result_counts_the_updated_rows() {
        let result = GroupResult::new(3);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({ "updated": 3 })
        );
        let parsed: GroupResult = serde_json::from_value(json!({ "updated": 3 })).unwrap();
        assert_eq!(parsed, result);
        assert!(serde_json::from_value::<GroupResult>(json!({})).is_err());
    }

    #[test]
    fn a_progress_frame_omits_what_it_does_not_know() {
        let frame = TaskProgress {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".parse().unwrap(),
            function: "note.export".into(),
            state: TaskState::Running,
            done: 2,
            total: None,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(&frame).unwrap(),
            json!({ "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "function": "note.export", "state": "running", "done": 2 })
        );
        let full: TaskProgress = serde_json::from_value(json!({
            "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3", "function": "note.export", "state": "done",
            "done": 3, "total": 3, "message": "exported"
        }))
        .unwrap();
        assert_eq!(full.state, TaskState::Done);
        assert_eq!(full.total, Some(3));
        assert_eq!(full.message.as_deref(), Some("exported"));
    }
}
