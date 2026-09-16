use serde_json::{Map, Value};

pub struct Json {
    rows: usize,
}

impl Json {
    pub fn begin() -> (Self, &'static str) {
        (Self { rows: 0 }, "[")
    }

    pub fn row(&mut self, columns: Option<&[String]>, row: Value) -> String {
        let row = match columns {
            Some(columns) => pick(columns, row),
            None => row,
        };
        let mut text = if self.rows == 0 {
            String::from("\n")
        } else {
            String::from(",\n")
        };
        self.rows += 1;
        text.push_str(&row.to_string());
        text
    }

    pub fn end(self) -> &'static str {
        if self.rows == 0 { "]\n" } else { "\n]\n" }
    }
}

fn pick(columns: &[String], row: Value) -> Value {
    let mut picked = Map::new();
    for key in columns {
        picked.insert(key.clone(), row.get(key).cloned().unwrap_or(Value::Null));
    }
    Value::Object(picked)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn rows_form_one_array_and_named_columns_pick_the_members_in_order() {
        let (mut json, head) = Json::begin();
        let mut text = head.to_owned();
        text.push_str(&json.row(None, json!({ "b": 2, "a": 1 })));
        text.push_str(&json.row(None, json!({ "a": 3 })));
        text.push_str(json.end());
        assert_eq!(text, "[\n{\"b\":2,\"a\":1},\n{\"a\":3}\n]\n");
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.as_array().map(Vec::len), Some(2));

        let (empty, head) = Json::begin();
        assert_eq!(format!("{head}{}", empty.end()), "[]\n");

        let columns = vec!["a".to_owned(), "missing".to_owned()];
        let (mut json, _) = Json::begin();
        assert_eq!(
            json.row(Some(&columns), json!({ "b": 2, "a": 1 })),
            "\n{\"a\":1,\"missing\":null}"
        );
    }
}
