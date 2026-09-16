use std::borrow::Cow;

use serde_json::Value;

const SEPARATOR: char = ',';
const RECORD_END: &str = "\r\n";

pub struct Csv;

impl Csv {
    pub fn field(text: &str) -> Cow<'_, str> {
        if text.contains([SEPARATOR, '"', '\n', '\r']) {
            Cow::Owned(format!("\"{}\"", text.replace('"', "\"\"")))
        } else {
            Cow::Borrowed(text)
        }
    }

    pub fn cell(value: &Value) -> String {
        match value {
            Value::Null => String::new(),
            Value::String(text) => text.clone(),
            Value::Bool(flag) => flag.to_string(),
            Value::Number(number) => number.to_string(),
            Value::Array(_) | Value::Object(_) => value.to_string(),
        }
    }

    pub fn record<'a>(fields: impl IntoIterator<Item = &'a str>) -> String {
        let mut record = String::new();
        for (index, field) in fields.into_iter().enumerate() {
            if index > 0 {
                record.push(SEPARATOR);
            }
            record.push_str(&Self::field(field));
        }
        record.push_str(RECORD_END);
        record
    }

    pub fn row(columns: &[String], row: &Value) -> Vec<String> {
        columns
            .iter()
            .map(|key| row.get(key).map(Self::cell).unwrap_or_default())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn fields_with_a_separator_a_quote_or_a_line_break_are_quoted_under_rfc_4180() {
        assert_eq!(Csv::field("plain"), "plain");
        assert_eq!(Csv::field(""), "");
        assert_eq!(Csv::field("milk, eggs"), "\"milk, eggs\"");
        assert_eq!(Csv::field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(Csv::field("two\nlines"), "\"two\nlines\"");
        assert_eq!(Csv::field("old\r\nmac"), "\"old\r\nmac\"");
        assert_eq!(
            Csv::record(["id", "title, long", "x"]),
            "id,\"title, long\",x\r\n"
        );
        assert_eq!(Csv::record(Vec::<&str>::new()), "\r\n");
    }

    #[test]
    fn cells_render_scalars_bare_null_as_empty_and_structures_as_json() {
        assert_eq!(Csv::cell(&json!(null)), "");
        assert_eq!(Csv::cell(&json!("text")), "text");
        assert_eq!(Csv::cell(&json!(42)), "42");
        assert_eq!(Csv::cell(&json!(1.5)), "1.5");
        assert_eq!(Csv::cell(&json!(true)), "true");
        assert_eq!(Csv::cell(&json!(["a", "b"])), "[\"a\",\"b\"]");
        assert_eq!(Csv::cell(&json!({ "k": 1 })), "{\"k\":1}");
        let columns = vec!["title".to_owned(), "due".to_owned(), "missing".to_owned()];
        let row = json!({ "title": "milk, eggs", "due": null, "extra": 1 });
        let cells = Csv::row(&columns, &row);
        assert_eq!(cells, ["milk, eggs", "", ""]);
        let fields: Vec<&str> = cells.iter().map(String::as_str).collect();
        assert_eq!(Csv::record(fields), "\"milk, eggs\",,\r\n");
    }
}
