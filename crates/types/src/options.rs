use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_OPTIONS_LIMIT: u32 = 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct OptionsQuery {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub search: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub scope: Map<String, Value>,
    pub limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl Default for OptionsQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            scope: Map::new(),
            limit: DEFAULT_OPTIONS_LIMIT,
            cursor: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choices {
    pub data: Vec<Choice>,
    pub more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Choice {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            hint: None,
            icon: None,
            group: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_options_query_takes_the_defaults() {
        let query: OptionsQuery = serde_json::from_value(json!({})).unwrap();
        assert_eq!(query, OptionsQuery::default());
        assert_eq!(query.limit, DEFAULT_OPTIONS_LIMIT);
        assert_eq!(
            serde_json::to_value(&query).unwrap(),
            json!({ "limit": 20 })
        );
        let query: OptionsQuery = serde_json::from_value(
            json!({ "search": "acme", "scope": { "zone_id": "z" }, "limit": 5, "cursor": "c" }),
        )
        .unwrap();
        assert_eq!(query.search, "acme");
        assert_eq!(query.scope["zone_id"], json!("z"));
        assert_eq!(query.limit, 5);
        assert_eq!(query.cursor.as_deref(), Some("c"));
    }

    #[test]
    fn choices_carry_value_label_and_the_optional_members_only_when_set() {
        let choices = Choices {
            data: vec![
                Choice::new("01J8A0000000000000000000A1", "acme").hint("reseller"),
                Choice::new("01J8A0000000000000000000A2", "bob"),
            ],
            more: true,
        };
        assert_eq!(
            serde_json::to_value(&choices).unwrap(),
            json!({
                "data": [
                    { "value": "01J8A0000000000000000000A1", "label": "acme", "hint": "reseller" },
                    { "value": "01J8A0000000000000000000A2", "label": "bob" }
                ],
                "more": true
            })
        );
        let back: Choices =
            serde_json::from_value(serde_json::to_value(&choices).unwrap()).unwrap();
        assert_eq!(back, choices);
        assert!(serde_json::from_value::<Choice>(json!({ "value": "x" })).is_err());
    }
}
