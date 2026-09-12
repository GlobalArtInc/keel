use std::path::Path;

use keel_types::{Descriptor, Envelope, Kind};
use serde_json::{Map, Value};

use crate::dispatcher::CallOptions;
use crate::socket::{Client, ClientError};

const COLUMN_GAP: &str = "  ";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Out {
    #[default]
    Json,
    Table,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub func: String,
    pub params: Value,
    pub out: Out,
    pub lang: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CtlError {
    #[error("{text:?} is not a key=value pair")]
    Pair { text: String },
    #[error("{text:?} has an empty key")]
    EmptyKey { text: String },
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("the envelope could not be printed: {0}")]
    Print(#[source] serde_json::Error),
}

pub fn params<'a>(pairs: impl IntoIterator<Item = &'a str>) -> Result<Value, CtlError> {
    let mut params = Map::new();
    for text in pairs {
        let Some((key, value)) = text.split_once('=') else {
            return Err(CtlError::Pair {
                text: text.to_owned(),
            });
        };
        if key.is_empty() {
            return Err(CtlError::EmptyKey {
                text: text.to_owned(),
            });
        }
        params.insert(key.to_owned(), value_of(value));
    }
    Ok(Value::Object(params))
}

fn value_of(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(parsed) => parsed,
        Err(_) => Value::String(text.to_owned()),
    }
}

pub async fn run(socket: &Path, request: &Request) -> Result<Output, CtlError> {
    let mut client = Client::connect(socket).await?;
    let envelope = client
        .call(
            &request.func,
            request.params.clone(),
            CallOptions::default(),
        )
        .await?;
    match (&envelope, request.out) {
        (_, Out::Json) => Ok(Output {
            ok: matches!(envelope, Envelope::Ok { .. }),
            text: serde_json::to_string_pretty(&envelope).map_err(CtlError::Print)?,
        }),
        (Envelope::Err { error }, Out::Table) => Ok(Output {
            ok: false,
            text: error_text(error),
        }),
        (Envelope::Ok { data, .. }, Out::Table) => {
            let descriptor = match client.describe(&request.func).await? {
                Envelope::Ok { data, .. } => Some(data),
                Envelope::Err { .. } => None,
            };
            let catalogue = match client.i18n(&request.lang).await? {
                Envelope::Ok { data, .. } => data.messages,
                Envelope::Err { .. } => Map::new(),
            };
            Ok(Output {
                ok: true,
                text: table(descriptor.as_ref(), &catalogue, data),
            })
        }
    }
}

fn error_text(error: &keel_types::CallError) -> String {
    let mut text = format!("error: {} {}", error.code.as_str(), error.key);
    if !error.params.is_empty() {
        text.push(' ');
        text.push_str(&Value::Object(error.params.clone()).to_string());
    }
    for field in &error.fields {
        text.push_str(&format!(
            "\n  {}: {}",
            field.path,
            field.key.as_deref().unwrap_or("")
        ));
        if !field.params.is_empty() {
            text.push(' ');
            text.push_str(&Value::Object(field.params.clone()).to_string());
        }
    }
    if let Some(request_id) = &error.request_id {
        text.push_str(&format!("\nrequest: {request_id}"));
    }
    text
}

pub fn table(
    descriptor: Option<&Descriptor>,
    catalogue: &Map<String, Value>,
    data: &Value,
) -> String {
    if let Some(descriptor) = descriptor
        && descriptor.kind == Kind::List
        && let Some(ui) = &descriptor.ui
        && let Some(rows) = data.get("data").and_then(Value::as_array)
    {
        let prefix = descriptor
            .name
            .rsplit_once('.')
            .map_or(descriptor.name.as_str(), |(prefix, _)| prefix);
        let columns: Vec<(&str, String)> = ui
            .columns
            .iter()
            .map(|column| {
                let key = format!("{prefix}.column.{}", column.key);
                let header = translate(catalogue, &key).unwrap_or(column.key.clone());
                (column.key.as_str(), header)
            })
            .collect();
        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|row| columns.iter().map(|(key, _)| cell(row.get(*key))).collect())
            .collect();
        let headers: Vec<String> = columns.into_iter().map(|(_, header)| header).collect();
        return grid(&headers, &cells);
    }
    match data {
        Value::Object(fields) => {
            let cells: Vec<Vec<String>> = fields
                .iter()
                .map(|(key, value)| vec![key.clone(), cell(Some(value))])
                .collect();
            grid(&[], &cells)
        }
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

fn translate(catalogue: &Map<String, Value>, key: &str) -> Option<String> {
    if let Some(Value::String(text)) = catalogue.get(key) {
        return Some(text.clone());
    }
    for (index, _) in key.match_indices('.') {
        let (head, tail) = (&key[..index], &key[index + 1..]);
        if let Some(Value::Object(child)) = catalogue.get(head)
            && let Some(found) = translate(child, tail)
        {
            return Some(found);
        }
    }
    None
}

fn cell(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn grid(headers: &[String], rows: &[Vec<String>]) -> String {
    let width = headers.len().max(rows.first().map_or(0, Vec::len));
    let mut widths = vec![0; width];
    for line in std::iter::once(headers).chain(rows.iter().map(Vec::as_slice)) {
        for (index, text) in line.iter().enumerate() {
            widths[index] = widths[index].max(text.chars().count());
        }
    }
    let mut out = String::new();
    if !headers.is_empty() {
        out.push_str(&padded(headers, &widths));
        out.push('\n');
    }
    for row in rows {
        out.push_str(&padded(row, &widths));
        out.push('\n');
    }
    out
}

fn padded(line: &[String], widths: &[usize]) -> String {
    let mut text = String::new();
    let last = line.len().saturating_sub(1);
    for (index, value) in line.iter().enumerate() {
        text.push_str(value);
        if index < last {
            let pad = widths[index].saturating_sub(value.chars().count());
            text.extend(std::iter::repeat_n(' ', pad));
            text.push_str(COLUMN_GAP);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn key_value_pairs_become_json_values_where_they_parse_and_strings_otherwise() {
        let params = params([
            "title=x",
            "ttl=300",
            "ratio=0.5",
            "id=01J9Q3W6B1R8N5X2K4T7M0V9Z3",
            "flag=true",
            "none=null",
            "quoted=\"300\"",
            "tags=[\"a\",\"b\"]",
            "sort={\"key\":\"name\",\"dir\":\"asc\"}",
            "empty=",
            "spaced=hello world",
            "eq=a=b",
            "title=y",
        ])
        .unwrap();
        assert_eq!(
            params,
            json!({
                "title": "y", "ttl": 300, "ratio": 0.5, "id": "01J9Q3W6B1R8N5X2K4T7M0V9Z3",
                "flag": true, "none": null, "quoted": "300", "tags": ["a", "b"],
                "sort": { "key": "name", "dir": "asc" }, "empty": "", "spaced": "hello world",
                "eq": "a=b"
            })
        );
        assert_eq!(super::params([]).unwrap(), json!({}));
        assert_eq!(
            super::params(["novalue"]).unwrap_err().to_string(),
            "\"novalue\" is not a key=value pair"
        );
        assert_eq!(
            super::params(["=x"]).unwrap_err().to_string(),
            "\"=x\" has an empty key"
        );
    }

    #[test]
    fn a_list_is_rendered_by_its_columns_with_translated_headers() {
        let descriptor: Descriptor = serde_json::from_value(json!({
            "keel": 1, "name": "probe.item.list", "module": "probe", "kind": "list", "mode": "read",
            "access": { "level": "user" }, "params": {}, "result": {},
            "ui": {
                "title": "probe.item.list.title",
                "columns": [
                    { "key": "id", "type": "text" },
                    { "key": "title", "type": "text" },
                    { "key": "weight", "type": "number" }
                ]
            }
        }))
        .unwrap();
        let catalogue =
            json!({ "probe": { "item": { "column": { "id": "Id", "title": "Headline" } } } })
                .as_object()
                .cloned()
                .unwrap();
        let data = json!({
            "data": [
                { "id": "a1", "title": "first", "weight": 10, "extra": true },
                { "id": "b22", "title": null, "weight": 2.5 }
            ],
            "total": 2
        });
        assert_eq!(
            table(Some(&descriptor), &catalogue, &data),
            "Id   Headline  weight\na1   first     10\nb22            2.5\n"
        );
        assert_eq!(
            table(
                Some(&descriptor),
                &catalogue,
                &json!({ "data": [], "total": 0 })
            ),
            "Id  Headline  weight\n"
        );
    }

    #[test]
    fn an_object_is_rendered_as_key_value_rows_and_anything_else_as_json() {
        assert_eq!(
            table(
                None,
                &Map::new(),
                &json!({ "id": "a1", "nested": { "x": 1 }, "none": null })
            ),
            "id      a1\nnested  {\"x\":1}\nnone    \n"
        );
        assert_eq!(table(None, &Map::new(), &json!([1, 2])), "[\n  1,\n  2\n]");
        assert_eq!(table(None, &Map::new(), &json!("plain")), "\"plain\"");
    }

    #[test]
    fn an_error_envelope_is_rendered_on_one_line_with_its_fields() {
        let error: keel_types::CallError = serde_json::from_value(json!({
            "code": "validation", "key": "keel.error.validation", "requestId": "req-1",
            "params": { "a": 1 },
            "fields": [ { "path": "kind", "code": "check.name", "key": "keel.check.name", "params": { "value": "x y" } } ]
        }))
        .unwrap();
        assert_eq!(
            error_text(&error),
            "error: validation keel.error.validation {\"a\":1}\n  kind: keel.check.name {\"value\":\"x y\"}\nrequest: req-1"
        );
    }
}
