use std::io::Write;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder};
use keel::sdk::serde_json;

use super::item;

pub const CLASS: &str = "export";
const MAX_PACE_MS: u64 = 2000;
pub const CSV_HEADER: &str = "id,title,kind,due,tags";
pub const LOG_PREFIX: &str = "exported";
pub const FILE_STEM: &str = "notes";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Export {
    #[serde(default)]
    pub format: ExportFormat,
    #[serde(default)]
    #[schemars(range(max = MAX_PACE_MS))]
    pub pace: u64,
}

#[derive(Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
struct Exported {
    pub exported: u64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(crate = "keel::sdk::serde", rename_all = "camelCase")]
struct Checkpoint {
    pub last_id: Option<String>,
    pub written: u64,
}

pub fn export() -> FunctionDef {
    function::<Export, Exported>("note.export")
        .kind(Kind::Action)
        .mode(Mode::Read)
        .access(Level::User, "note.read")
        .unowned()
        .class(CLASS)
        .resumable()
        .ui(ActionScreen::new("note.export.title").result(ScreenResult::Task))
        .handler(|call: Call<Export>| async move {
            let task = call
                .task()
                .ok_or_else(|| KeelError::internal("note.error.export_inline"))?;
            let pace = Duration::from_millis(call.params().pace);
            let format = call.params().format;
            let checkpoint = task.resume_from::<Checkpoint>()?.unwrap_or_default();
            let scoped = call.account_scope().apply(item::Entity::find());
            let total = scoped.clone().count(call.db()).await?;
            let mut pending = scoped.order_by_asc(item::Column::Id);
            if let Some(last) = &checkpoint.last_id {
                pending = pending.filter(item::Column::Id.gt(last.as_str()));
            }
            let notes = pending.all(call.db()).await?;
            let mut output = task.output()?;
            let mut written = checkpoint.written;
            if written == 0 && format == ExportFormat::Csv {
                writeln!(output, "{CSV_HEADER}").map_err(output_failed)?;
            }
            for note in notes {
                if task.is_cancelled() {
                    return Err(task.cancelled());
                }
                writeln!(output, "{}", record(format, &note)?).map_err(output_failed)?;
                output.flush().map_err(output_failed)?;
                written += 1;
                task.log(&format!("{LOG_PREFIX} {}", note.title)).await?;
                task.progress(written, Some(total), None).await?;
                task.checkpoint(&Checkpoint {
                    last_id: Some(note.id),
                    written,
                })
                .await?;
                if !pace.is_zero() {
                    keel::sdk::tokio::time::sleep(pace).await;
                }
            }
            task.stored(file_of(format));
            Ok(Exported { exported: written })
        })
}

pub fn file_of(format: ExportFormat) -> DownloadFile {
    match format {
        ExportFormat::Csv => {
            DownloadFile::new(format!("{FILE_STEM}.csv"), "text/csv; charset=utf-8")
        }
        ExportFormat::Json => DownloadFile::new(format!("{FILE_STEM}.json"), "application/json"),
    }
}

fn record(format: ExportFormat, note: &item::Model) -> Result<String> {
    match format {
        ExportFormat::Csv => Ok([
            csv_field(&note.id),
            csv_field(&note.title),
            csv_field(&note.kind),
            csv_field(&note.due.map(|due| due.to_string()).unwrap_or_default()),
            csv_field(&note.tags.join(" ")),
        ]
        .join(",")),
        ExportFormat::Json => Ok(serde_json::to_string(note)?),
    }
}

pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

fn output_failed(source: std::io::Error) -> KeelError {
    KeelError::internal("note.error.export_output").source(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_when_they_carry_a_separator_a_quote_or_a_line_break() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field(""), "");
    }

    #[test]
    fn a_record_is_one_csv_line_or_one_json_object() {
        let note = item::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".into(),
            account_id: "01J8A0000000000000000000A1".into(),
            title: "milk, eggs".into(),
            body: String::new(),
            kind: "todo".into(),
            tags: vec!["shop".into(), "home".into()],
            due: Some(keel::sdk::chrono::NaiveDate::from_ymd_opt(2026, 9, 20).expect("a date")),
            created_at: Default::default(),
            updated_at: Default::default(),
        };
        assert_eq!(
            record(ExportFormat::Csv, &note).expect("csv"),
            "01J9Q3W6B1R8N5X2K4T7M0V9Z3,\"milk, eggs\",todo,2026-09-20,shop home"
        );
        let json: Value =
            serde_json::from_str(&record(ExportFormat::Json, &note).expect("json")).expect("json");
        assert_eq!(json["title"], json!("milk, eggs"));
        assert_eq!(json["tags"], json!(["shop", "home"]));
        assert_eq!(CSV_HEADER.split(',').count(), 5);
    }

    #[test]
    fn the_pace_is_bounded_by_the_schema_and_defaults_to_no_wait() {
        let descriptor =
            serde_json::to_value(export().descriptor().expect("compiles")).expect("json");
        let pace = &descriptor["params"]["properties"]["pace"];
        assert_eq!(pace["maximum"], json!(MAX_PACE_MS));
        assert_eq!(pace["default"], json!(0));
        assert_eq!(
            descriptor["params"]["properties"]["format"]["default"],
            json!("csv")
        );
        assert_eq!(
            descriptor["ui"],
            json!({ "title": "note.export.title", "result": "task" })
        );
        assert_eq!(descriptor["mode"], json!("read"));
        assert_eq!(descriptor["kind"], json!("action"));
        assert!(export().is_resumable());
        assert_eq!(export().class(), CLASS);
    }

    #[test]
    fn the_stored_file_is_named_and_typed_after_the_format() {
        assert_eq!(
            file_of(ExportFormat::Csv),
            DownloadFile::new("notes.csv", "text/csv; charset=utf-8")
        );
        assert_eq!(
            file_of(ExportFormat::Json),
            DownloadFile::new("notes.json", "application/json")
        );
    }

    #[test]
    fn the_checkpoint_starts_before_the_first_note() {
        let fresh = Checkpoint::default();
        assert_eq!(fresh.last_id, None);
        assert_eq!(fresh.written, 0);
        let value = serde_json::to_value(Checkpoint {
            last_id: Some("01J".into()),
            written: 2,
        })
        .expect("serialises");
        assert_eq!(value, json!({ "lastId": "01J", "written": 2 }));
    }
}
