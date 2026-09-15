use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::id::Id;

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
