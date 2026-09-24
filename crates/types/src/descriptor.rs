use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::check::Check;
use crate::is_false;
use crate::level::Level;
use crate::ui::ScreenUi;

pub const DESCRIPTOR_VERSION: u32 = 1;
pub const X_KEEL: &str = "x-keel";
pub const X_UI: &str = "x-ui";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Descriptor {
    pub keel: u32,
    pub name: String,
    pub module: String,
    pub kind: Kind,
    pub mode: Mode,
    pub access: Access,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[schemars(schema_with = "schema_document")]
    pub params: Schema,
    #[schemars(schema_with = "schema_document")]
    pub result: Schema,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<ScreenUi>,
}

fn schema_document(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": ["object", "boolean"] })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    List,
    Get,
    Form,
    Action,
    Wizard,
    Report,
    Options,
    Stream,
    Download,
    Upload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Access {
    pub level: Level,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemberAccess {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Control {
    Input,
    Password,
    Select,
    Toggle,
    Switcher,
    Slider,
    Date,
    Time,
    File,
    List,
    Code,
    Tags,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KeelAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub once: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    #[serde(default, rename = "checkBy", skip_serializing_if = "Option::is_none")]
    pub check_by: Option<CheckBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileAttrs>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckBy {
    pub field: String,
    pub cases: BTreeMap<String, Check>,
}

impl CheckBy {
    pub fn select(&self, siblings: &Map<String, Value>) -> Option<Check> {
        siblings
            .get(&self.field)
            .and_then(Value::as_str)
            .and_then(|case| self.cases.get(case))
            .copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileAttrs {
    pub max_bytes: u64,
    pub types: Vec<String>,
}

impl FileAttrs {
    pub fn new<I, S>(max_bytes: u64, types: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            max_bytes,
            types: types.into_iter().map(Into::into).collect(),
        }
    }

    pub fn admits(&self, content_type: &str) -> bool {
        let essence = content_type.split(';').next().unwrap_or_default().trim();
        self.types
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(essence))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UiAttrs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<Control>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub generate: bool,
}

impl KeelAttrs {
    pub fn of(field: &Schema) -> Result<Option<Self>, serde_json::Error> {
        attrs(field, X_KEEL)
    }
}

impl UiAttrs {
    pub fn of(field: &Schema) -> Result<Option<Self>, serde_json::Error> {
        attrs(field, X_UI)
    }
}

fn attrs<T: DeserializeOwned>(field: &Schema, space: &str) -> Result<Option<T>, serde_json::Error> {
    field
        .get(space)
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_kind_mode_and_control_fail_to_deserialise() {
        assert!(serde_json::from_value::<Kind>(json!("table")).is_err());
        assert!(serde_json::from_value::<Kind>(json!("List")).is_err());
        assert!(serde_json::from_value::<Mode>(json!("readwrite")).is_err());
        assert!(serde_json::from_value::<Control>(json!("textarea")).is_err());
    }

    #[test]
    fn kind_covers_the_closed_vocabulary() {
        let names: Vec<serde_json::Value> = [
            Kind::List,
            Kind::Get,
            Kind::Form,
            Kind::Action,
            Kind::Wizard,
            Kind::Report,
            Kind::Options,
            Kind::Stream,
            Kind::Download,
            Kind::Upload,
        ]
        .into_iter()
        .map(|kind| serde_json::to_value(kind).unwrap())
        .collect();
        assert_eq!(
            names,
            json!([
                "list", "get", "form", "action", "wizard", "report", "options", "stream",
                "download", "upload"
            ])
            .as_array()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn attribute_spaces_are_read_from_a_field_schema() {
        let field = json_schema!({
            "type": "string",
            "x-keel": { "level": "reseller", "options": "keel.account.options", "check": "domain", "secret": true },
            "x-ui": { "control": "tags", "generate": true, "order": 3 }
        });
        let keel = KeelAttrs::of(&field).unwrap().unwrap();
        assert_eq!(keel.level, Some(Level::Reseller));
        assert_eq!(keel.options.as_deref(), Some("keel.account.options"));
        assert_eq!(keel.check, Some(Check::Domain));
        assert!(keel.secret);
        assert!(!keel.once);
        let shown_once = json_schema!({ "type": "string", "x-keel": { "once": true } });
        let keel = KeelAttrs::of(&shown_once).unwrap().unwrap();
        assert!(keel.once);
        assert!(!keel.secret);
        assert_eq!(
            serde_json::to_value(&keel).unwrap(),
            json!({ "once": true })
        );
        let ui = UiAttrs::of(&field).unwrap().unwrap();
        assert_eq!(ui.control, Some(Control::Tags));
        assert!(ui.generate);
        assert_eq!(ui.order, Some(3));
        let plain = json_schema!({ "type": "string" });
        assert_eq!(KeelAttrs::of(&plain).unwrap(), None);
        assert_eq!(UiAttrs::of(&plain).unwrap(), None);
    }

    #[test]
    fn a_file_attribute_names_the_size_cap_and_the_admitted_types() {
        let field = json_schema!({
            "type": "string",
            "x-keel": { "file": { "maxBytes": 65536, "types": ["image/svg+xml"] } },
            "x-ui": { "control": "file" }
        });
        let keel = KeelAttrs::of(&field).unwrap().unwrap();
        let file = keel.file.clone().expect("the file attribute");
        assert_eq!(file, FileAttrs::new(65536, ["image/svg+xml"]));
        assert!(file.admits("image/svg+xml"));
        assert!(file.admits("IMAGE/SVG+XML; charset=utf-8"));
        assert!(!file.admits("image/png"));
        assert!(!file.admits(""));
        assert_eq!(
            serde_json::to_value(&keel).unwrap(),
            json!({ "file": { "maxBytes": 65536, "types": ["image/svg+xml"] } })
        );
        assert_eq!(
            UiAttrs::of(&field).unwrap().unwrap().control,
            Some(Control::File)
        );
        let short = json_schema!({ "x-keel": { "file": { "maxBytes": 1 } } });
        assert!(KeelAttrs::of(&short).is_err());
    }

    #[test]
    fn a_check_chosen_by_a_sibling_names_the_field_and_a_preset_per_value() {
        let field = json_schema!({
            "type": "string",
            "x-keel": { "checkBy": { "field": "type", "cases": { "A": "ipv4", "TXT": "quoted" } } }
        });
        let keel = KeelAttrs::of(&field).unwrap().unwrap();
        let by = keel.check_by.clone().expect("the checkBy attribute");
        assert_eq!(by.field, "type");
        let siblings = |value: Value| value.as_object().cloned().unwrap_or_default();
        assert_eq!(
            by.select(&siblings(json!({ "type": "A" }))),
            Some(Check::Ipv4)
        );
        assert_eq!(
            by.select(&siblings(json!({ "type": "TXT" }))),
            Some(Check::Quoted)
        );
        assert_eq!(by.select(&siblings(json!({ "type": "MX" }))), None);
        assert_eq!(by.select(&siblings(json!({ "type": 1 }))), None);
        assert_eq!(by.select(&siblings(json!({}))), None);
        assert_eq!(
            serde_json::to_value(&keel).unwrap(),
            json!({ "checkBy": { "field": "type", "cases": { "A": "ipv4", "TXT": "quoted" } } })
        );
        let unknown = json_schema!({ "x-keel": { "checkBy": { "field": "type", "cases": { "A": "phone" } } } });
        assert!(KeelAttrs::of(&unknown).is_err());
    }

    #[test]
    fn an_unknown_check_or_level_in_x_keel_is_an_error() {
        let field = json_schema!({ "x-keel": { "check": "phone" } });
        assert!(KeelAttrs::of(&field).is_err());
        let field = json_schema!({ "x-keel": { "level": "root" } });
        assert!(KeelAttrs::of(&field).is_err());
    }

    #[test]
    fn descriptor_without_key_and_ui_is_minimal_on_the_wire() {
        let descriptor = Descriptor {
            keel: DESCRIPTOR_VERSION,
            name: "keel.account.options".into(),
            module: "keel".into(),
            kind: Kind::Options,
            mode: Mode::Read,
            access: Access {
                level: Level::Reseller,
                permission: None,
                feature: None,
            },
            key: None,
            params: json_schema!({ "type": "object" }),
            result: json_schema!({ "type": "null" }),
            ui: None,
        };
        assert_eq!(
            serde_json::to_value(&descriptor).unwrap(),
            json!({
                "keel": 1, "name": "keel.account.options", "module": "keel", "kind": "options", "mode": "read",
                "access": { "level": "reseller" }, "params": { "type": "object" }, "result": { "type": "null" }
            })
        );
    }

    #[test]
    fn descriptor_without_result_fails_to_deserialise() {
        let wire = json!({
            "keel": 1, "name": "keel.account.options", "module": "keel", "kind": "options", "mode": "read",
            "access": { "level": "reseller" }, "params": { "type": "object" }
        });
        assert!(serde_json::from_value::<Descriptor>(wire).is_err());
    }
}
