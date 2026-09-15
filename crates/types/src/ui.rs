use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::descriptor::MemberAccess;
use crate::expr::Expr;
use crate::is_false;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenUi {
    pub title: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub row_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<Empty>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalidate_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<ScreenPage>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, FieldUi>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub multi: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ScreenResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Column {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: ColumnType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Link>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sortable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<Total>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<BTreeMap<String, Tone>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<ColumnFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<InlineEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<MemberAccess>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Text,
    Number,
    Link,
    Status,
    Toggle,
    Date,
    Datetime,
    Boolean,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Success,
    Warning,
    Danger,
    Info,
    Neutral,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Total {
    Sum,
    Avg,
    Min,
    Max,
    Count,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColumnFilter {
    Options { options: Vec<String> },
    Remote { remote: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InlineEdit {
    pub function: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Link {
    pub screen: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Action {
    pub id: String,
    pub role: ActionRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Confirm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden_if: Option<Expr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<ExportFormat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ActionRole {
    New,
    Edit,
    Delete,
    Group,
    Button,
    Child,
    External,
    Export,
    Upload,
    Download,
    Refresh,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Confirm {
    Simple(bool),
    Detailed(ConfirmDetails),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfirmDetails {
    #[serde(default, skip_serializing_if = "is_false")]
    pub destructive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<ConfirmInput>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmInput {
    Count,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Csv,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Empty {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenPage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_if: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validate: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldUi {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_if: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_if: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_if: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recompute: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScreenResult {
    Redirect,
    Task,
    Download,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_values_of_the_ui_vocabulary_fail_to_deserialise() {
        assert!(serde_json::from_value::<ColumnType>(json!("money")).is_err());
        assert!(serde_json::from_value::<ActionRole>(json!("open")).is_err());
        assert!(serde_json::from_value::<Tone>(json!("green")).is_err());
        assert!(serde_json::from_value::<Total>(json!("median")).is_err());
        assert!(serde_json::from_value::<ExportFormat>(json!("xml")).is_err());
        assert!(serde_json::from_value::<ConfirmInput>(json!("name")).is_err());
        assert!(serde_json::from_value::<ScreenResult>(json!("reload")).is_err());
        assert!(serde_json::from_value::<ColumnFilter>(json!({ "values": ["a"] })).is_err());
    }

    #[test]
    fn confirm_is_either_a_flag_or_details() {
        assert_eq!(
            serde_json::from_value::<Confirm>(json!(true)).unwrap(),
            Confirm::Simple(true)
        );
        let detailed = serde_json::from_value::<Confirm>(
            json!({ "destructive": true, "items": "name", "input": "count" }),
        )
        .unwrap();
        assert_eq!(
            detailed,
            Confirm::Detailed(ConfirmDetails {
                destructive: true,
                items: Some("name".into()),
                input: Some(ConfirmInput::Count)
            })
        );
        assert_eq!(
            serde_json::to_value(Confirm::Detailed(ConfirmDetails::default())).unwrap(),
            json!({})
        );
    }

    #[test]
    fn form_ui_round_trips() {
        let form = ScreenUi {
            title: "dns.zone.edit.title".into(),
            read: Some("dns.zone.get".into()),
            pages: vec![
                ScreenPage {
                    id: "main".into(),
                    fields: vec!["name".into(), "kind".into()],
                    show_if: None,
                    validate: None,
                },
                ScreenPage {
                    id: "transfer".into(),
                    fields: vec!["master_ips".into()],
                    show_if: Some(Expr::parse("kind == 'slave'").unwrap()),
                    validate: None,
                },
            ],
            fields: BTreeMap::from([
                (
                    "name".to_owned(),
                    FieldUi {
                        readonly_if: Some(Expr::parse("$mode == 'edit'").unwrap()),
                        ..Default::default()
                    },
                ),
                (
                    "ttl".to_owned(),
                    FieldUi {
                        recompute: Some("dns.zone.recompute".into()),
                        ..Default::default()
                    },
                ),
            ]),
            multi: vec!["ttl".into()],
            ..Default::default()
        };
        let wire = serde_json::to_value(&form).unwrap();
        assert_eq!(
            wire,
            json!({
                "title": "dns.zone.edit.title",
                "read": "dns.zone.get",
                "pages": [
                    { "id": "main", "fields": ["name", "kind"] },
                    { "id": "transfer", "fields": ["master_ips"], "show_if": { "eq": ["kind", "slave"] } }
                ],
                "fields": {
                    "name": { "readonly_if": { "eq": ["$mode", "edit"] } },
                    "ttl": { "recompute": "dns.zone.recompute" }
                },
                "multi": ["ttl"]
            })
        );
        assert_eq!(serde_json::from_value::<ScreenUi>(wire).unwrap(), form);
    }
}
