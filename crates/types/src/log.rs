use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LogLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
    pub level: LogLevel,
    pub text: String,
}

impl LogLine {
    pub fn new(level: LogLevel, text: impl Into<String>) -> Self {
        Self {
            at: None,
            level,
            text: text.into(),
        }
    }

    pub fn at(mut self, at: DateTime<Utc>) -> Self {
        self.at = Some(at);
        self
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_line_serialises_its_time_only_when_it_has_one() {
        let bare = LogLine::new(LogLevel::Warn, "disk almost full");
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({ "level": "warn", "text": "disk almost full" })
        );
        let at = Utc
            .with_ymd_and_hms(2026, 9, 26, 12, 0, 0)
            .single()
            .unwrap();
        let timed = LogLine::new(LogLevel::Info, "applied").at(at);
        let value = serde_json::to_value(&timed).unwrap();
        assert_eq!(value["at"], json!("2026-09-26T12:00:00Z"));
        assert_eq!(serde_json::from_value::<LogLine>(value).unwrap(), timed);
    }

    #[test]
    fn levels_order_from_trace_to_error() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Warn < LogLevel::Error);
    }
}
