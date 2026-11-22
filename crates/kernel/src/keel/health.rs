use futures::StreamExt;
use keel_sdk::FunctionBuilder;
use keel_sdk::prelude::*;
use keel_sdk::types::{FilterOp, FilterValue};

use crate::doctor::{Database, Handle, Status};
use crate::logs::{self, Entry, LogLevel};

pub(super) const DOCTOR_RUN: &str = "keel.doctor.run";
pub const LOGS_TAIL: &str = "keel.logs.tail";
pub(super) const LOGS_STREAM: &str = "keel.logs.stream";
pub(super) const LOGS_MENU_PATH: &str = "monitor/logs/server";
pub(super) const LOGS_MENU_ICON: &str = "file-text";
pub(super) const MENU_PATH: &str = "monitor/health";
pub(super) const MENU_ICON: &str = "activity";
const DEFAULT_TAIL: u32 = 200;
const ERROR_READ: &str = "keel.error.doctor_not_started";
const STATUS_KEY: &str = "status";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(super) struct CheckRow {
    pub(super) check: String,
    pub(super) status: Status,
    pub(super) message: String,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(super) struct LogsTail {
    #[serde(default)]
    #[schemars(range(min = 1, max = 10_000))]
    pub(super) limit: Option<u32>,
    #[serde(default)]
    pub(super) level: Option<LogLevel>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub(super) struct LogsStream {}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LogsPage {
    pub(super) entries: Vec<Entry>,
    pub(super) total: u32,
    pub(super) capacity: u32,
    pub(super) dropped: u64,
}

fn status_admitted(filters: &[keel_sdk::types::Filter], status: Status) -> bool {
    filters
        .iter()
        .filter(|filter| filter.key == STATUS_KEY)
        .all(|filter| {
            let matches = match &filter.value {
                FilterValue::One(value) => value == status.as_str(),
                FilterValue::Many(values) => values.iter().any(|value| value == status.as_str()),
            };
            match filter.op {
                FilterOp::Ne => !matches,
                _ => matches,
            }
        })
}

pub(super) fn doctor_run() -> FunctionDef {
    function::<ListQuery, Page<CheckRow>>(DOCTOR_RUN)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .ui(List::new("keel.doctor.run.title")
            .column(Column::new("check", ColumnType::Text).primary())
            .column(
                Column::new(STATUS_KEY, ColumnType::Status)
                    .map([
                        (Status::Ok.as_str(), Tone::Success),
                        (Status::Warn.as_str(), Tone::Warning),
                        (Status::Fail.as_str(), Tone::Danger),
                    ])
                    .filter_options([
                        Status::Ok.as_str(),
                        Status::Warn.as_str(),
                        Status::Fail.as_str(),
                    ]),
            )
            .column(Column::new("message", ColumnType::Text))
            .action(Action::new("refresh", ActionRole::Refresh))
            .empty("keel.doctor.empty", None))
        .handler(|call: Call<ListQuery>| async move {
            let doctor = call.require::<Handle>()?.doctor()?;
            let db = call
                .db()
                .connection()
                .ok_or_else(|| KeelError::unavailable(ERROR_READ))?;
            let report = doctor.run(&Database::Connected(db)).await;
            let rows: Vec<CheckRow> = report
                .checks
                .into_iter()
                .filter(|check| status_admitted(&call.params().filters, check.status))
                .map(|check| CheckRow {
                    check: check.id,
                    status: check.status,
                    message: check.message,
                })
                .collect();
            let total = rows.len() as u64;
            Ok(Page::new(rows, total))
        })
}

pub(super) fn logs_tail() -> FunctionDef {
    function::<LogsTail, LogsPage>(LOGS_TAIL)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .handler(|call: Call<LogsTail>| async move {
            let params = call.params();
            let ring = logs::ring();
            let limit = usize::try_from(params.limit.unwrap_or(DEFAULT_TAIL))
                .unwrap_or(logs::CAPACITY)
                .min(ring.capacity());
            Ok(LogsPage {
                entries: ring.tail(limit, params.level),
                total: u32::try_from(ring.len()).unwrap_or(u32::MAX),
                capacity: u32::try_from(ring.capacity()).unwrap_or(u32::MAX),
                dropped: ring.dropped(),
            })
        })
}

pub(super) fn logs_stream() -> FunctionBuilder<LogsStream, LogLine> {
    function::<LogsStream, LogLine>(LOGS_STREAM)
        .level(Level::Admin)
        .unowned()
        .ui(ObjectScreen::new("keel.logs.stream.title"))
}

pub(super) async fn follow_logs(_: Call<LogsStream>) -> Result<Lines> {
    let backlog = usize::try_from(DEFAULT_TAIL).unwrap_or(logs::CAPACITY);
    Ok(Lines::new(
        logs::ring().follow(backlog).map(|entry| entry.line()),
    ))
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;
    use keel_sdk::types::Filter;

    use super::*;

    fn filter(op: FilterOp, value: FilterValue) -> Filter {
        Filter {
            key: STATUS_KEY.to_owned(),
            op,
            value,
        }
    }

    #[test]
    fn the_status_filter_of_the_health_screen_admits_by_equality() {
        assert!(status_admitted(&[], Status::Ok));
        let fail = [filter(FilterOp::Eq, FilterValue::One("fail".into()))];
        assert!(status_admitted(&fail, Status::Fail));
        assert!(!status_admitted(&fail, Status::Ok));
        let not_ok = [filter(FilterOp::Ne, FilterValue::One("ok".into()))];
        assert!(status_admitted(&not_ok, Status::Warn));
        assert!(!status_admitted(&not_ok, Status::Ok));
        let many = [filter(
            FilterOp::Eq,
            FilterValue::Many(vec!["warn".into(), "fail".into()]),
        )];
        assert!(status_admitted(&many, Status::Warn));
        assert!(!status_admitted(&many, Status::Ok));
        let other = [Filter {
            key: "check".into(),
            op: FilterOp::Eq,
            value: FilterValue::One("units".into()),
        }];
        assert!(status_admitted(&other, Status::Ok));
    }

    #[test]
    fn the_health_screen_is_an_admin_list_with_a_status_column_and_the_tail_is_an_admin_get() {
        let run = doctor_run();
        assert_eq!(run.name(), DOCTOR_RUN);
        assert_eq!(run.kind(), Kind::List);
        assert_eq!(run.access().level, Level::Admin);
        let descriptor = serde_json::to_value(run.descriptor().expect("compiles")).unwrap();
        let columns: Vec<&str> = descriptor["ui"]["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column["key"].as_str().unwrap())
            .collect();
        assert_eq!(columns, ["check", "status", "message"]);
        assert_eq!(descriptor["ui"]["columns"][1]["map"]["fail"], "danger");
        assert_eq!(descriptor["ui"]["actions"][0]["role"], "refresh");
        let tail = logs_tail();
        assert_eq!(tail.name(), LOGS_TAIL);
        assert_eq!(tail.kind(), Kind::Get);
        assert_eq!(tail.access().level, Level::Admin);
        let descriptor = serde_json::to_value(tail.descriptor().expect("compiles")).unwrap();
        assert_eq!(
            descriptor["params"]["properties"]["limit"]["maximum"],
            10_000
        );
        assert!(
            descriptor["params"]["properties"]["level"]["anyOf"].is_array()
                || descriptor["params"]["properties"]["level"]["enum"].is_array(),
            "{}",
            descriptor["params"]
        );
        let params: LogsTail = serde_json::from_value(json!({})).unwrap();
        assert_eq!(params.limit, None);
        assert_eq!(params.level, None);
        let params: LogsTail =
            serde_json::from_value(json!({ "limit": 5, "level": "warn" })).unwrap();
        assert_eq!(params.limit, Some(5));
        assert_eq!(params.level, Some(LogLevel::Warn));
    }
}
