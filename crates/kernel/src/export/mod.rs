mod csv;
mod function;
mod json;

use keel_sdk::KeelError;
use keel_types::{Descriptor, ExportFormat};
use serde_json::{Map, Value};

use crate::http::catalogue;
use crate::loader::i18n::find;
use crate::loader::keys::{SHARED_PREFIX, entity_prefix};
use crate::registry::Registry;

pub use csv::Csv;
pub use function::{CLASS, EXPORT_TASK_ROWS, FUNCTION, function};

pub const COLUMNS_KEY: &str = "keel.error.export_columns";
pub const CSV_TYPE: &str = "text/csv; charset=utf-8";
pub const JSON_TYPE: &str = "application/json";

const SHARED_DEPTHS: [usize; 2] = [2, 1];

fn extension(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
    }
}

fn content_type(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Csv => CSV_TYPE,
        ExportFormat::Json => JSON_TYPE,
    }
}

fn columns_of(
    descriptor: &Descriptor,
    requested: Option<&[String]>,
) -> Result<Vec<String>, KeelError> {
    let visible: Vec<String> = descriptor
        .ui
        .as_ref()
        .map(|ui| ui.columns.iter().map(|column| column.key.clone()).collect())
        .unwrap_or_default();
    let Some(requested) = requested else {
        return Ok(visible);
    };
    let mut columns = Vec::with_capacity(requested.len());
    for key in requested {
        if !visible.contains(key) {
            return Err(KeelError::validation(COLUMNS_KEY)
                .field("columns")
                .param("column", key.as_str())
                .param("function", descriptor.name.as_str()));
        }
        if !columns.contains(key) {
            columns.push(key.clone());
        }
    }
    Ok(columns)
}

fn labels(registry: &Registry, lang: &str, function: &str, keys: &[String]) -> Vec<String> {
    let messages = catalogue(registry, lang).messages;
    keys.iter()
        .map(|key| column_label(&messages, function, key))
        .collect()
}

fn column_label(messages: &Map<String, Value>, function: &str, key: &str) -> String {
    let own = format!("{}.column.{key}", entity_prefix(function));
    if let Some(text) = text_of(messages, &own) {
        return text.to_owned();
    }
    shared_keys(&own)
        .iter()
        .find_map(|shared| text_of(messages, shared))
        .map_or(own, str::to_owned)
}

fn shared_keys(key: &str) -> Vec<String> {
    let segments: Vec<&str> = key.split('.').collect();
    SHARED_DEPTHS
        .iter()
        .filter(|depth| segments.len() > **depth)
        .map(|depth| format!("{SHARED_PREFIX}.{}", segments[*depth..].join(".")))
        .filter(|shared| shared != key)
        .collect()
}

fn text_of<'a>(messages: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    find(messages, key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use keel_types::descriptor::MemberAccess;
    use keel_types::{Access, Column, ColumnType, DESCRIPTOR_VERSION, Kind, Level, Mode, ScreenUi};
    use schemars::json_schema;
    use serde_json::json;

    use super::*;

    fn descriptor(columns: &[(&str, Option<Level>)]) -> Descriptor {
        Descriptor {
            keel: DESCRIPTOR_VERSION,
            name: "note.list".into(),
            module: "note".into(),
            kind: Kind::List,
            mode: Mode::Read,
            access: Access {
                level: Level::User,
                permission: None,
                feature: None,
            },
            key: None,
            params: json_schema!({}),
            result: json_schema!({}),
            ui: Some(ScreenUi {
                title: "note.list.title".into(),
                columns: columns
                    .iter()
                    .map(|(key, level)| Column {
                        key: (*key).to_owned(),
                        kind: ColumnType::Text,
                        to: None,
                        sortable: false,
                        primary: false,
                        total: None,
                        map: None,
                        filter: None,
                        edit: None,
                        access: level.map(|level| MemberAccess {
                            level: Some(level),
                            permission: None,
                        }),
                    })
                    .collect(),
                ..ScreenUi::default()
            }),
        }
    }

    #[test]
    fn the_columns_default_to_the_projected_list_and_a_named_subset_must_be_part_of_it() {
        let descriptor = descriptor(&[("title", None), ("accountId", Some(Level::Reseller))]);
        assert_eq!(
            columns_of(&descriptor, None).unwrap(),
            ["title", "accountId"]
        );
        let picked = vec![
            "accountId".to_owned(),
            "title".to_owned(),
            "title".to_owned(),
        ];
        assert_eq!(
            columns_of(&descriptor, Some(&picked)).unwrap(),
            ["accountId", "title"]
        );
        let unknown = vec!["body".to_owned()];
        let error = columns_of(&descriptor, Some(&unknown)).unwrap_err();
        assert_eq!(error.key(), COLUMNS_KEY);
        assert_eq!(error.field_path(), Some("columns"));
        assert_eq!(error.params()["column"], json!("body"));
        assert_eq!(error.status(), 422);
        let bare = Descriptor {
            ui: None,
            ..descriptor
        };
        assert!(columns_of(&bare, None).unwrap().is_empty());
        assert!(columns_of(&bare, Some(&unknown)).is_err());
    }

    #[test]
    fn a_column_key_falls_back_to_the_shared_keys_as_the_shell_does() {
        assert_eq!(
            shared_keys("note.column.title"),
            ["keel.title", "keel.column.title"]
        );
        assert_eq!(
            shared_keys("dns.zone.column.name"),
            ["keel.column.name", "keel.zone.column.name"]
        );
        assert_eq!(shared_keys("keel.column.id"), ["keel.id"]);
        assert!(shared_keys("bare").is_empty());
        let messages = json!({
            "keel": { "name": "Name", "column": { "name": "Full name", "qty": "Quantity" } },
            "note": { "column": { "title": "Title" } }
        });
        let messages = messages.as_object().unwrap();
        assert_eq!(column_label(messages, "note.list", "title"), "Title");
        assert_eq!(column_label(messages, "note.list", "name"), "Name");
        assert_eq!(column_label(messages, "note.list", "qty"), "Quantity");
        assert_eq!(
            column_label(messages, "note.list", "body"),
            "note.column.body"
        );
        assert_eq!(extension(ExportFormat::Csv), "csv");
        assert_eq!(extension(ExportFormat::Json), "json");
        assert_eq!(content_type(ExportFormat::Csv), CSV_TYPE);
        assert_eq!(content_type(ExportFormat::Json), JSON_TYPE);
    }
}
