use keel_types::{Check, Control, Id, KeelAttrs, Level, UiAttrs};
use schemars::{JsonSchema, Schema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ZoneKind {
    Master,
    Slave,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct Secret(String);

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ZoneEdit {
    pub id: Option<Id>,
    #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
    pub name: String,
    pub kind: ZoneKind,
    #[schemars(extend("x-keel" = json!({ "check": "ip" }), "x-ui" = json!({ "control": "tags" })))]
    pub master_ips: Vec<String>,
    pub ttl: u32,
    #[schemars(extend("x-keel" = json!({ "secret": true }), "x-ui" = json!({ "control": "password", "generate": true })))]
    pub tsig_key: Option<Secret>,
    #[schemars(extend("x-keel" = json!({ "level": "reseller", "options": "keel.account.options" })))]
    pub account: Option<Id>,
}

fn property(schema: &Schema, name: &str) -> Schema {
    let value = schema
        .get("properties")
        .and_then(|properties| properties.get(name))
        .unwrap_or_else(|| panic!("property {name}"))
        .clone();
    serde_json::from_value(value).expect("property is a schema")
}

#[test]
fn zone_edit_schema_carries_x_keel_and_x_ui_exactly_as_written() {
    let schema = schema_for!(ZoneEdit);
    let value = schema.as_value();

    assert_eq!(
        value["properties"]["name"]["x-keel"],
        json!({ "check": "domain" })
    );
    assert_eq!(value["properties"]["name"].get("x-ui"), None);
    assert_eq!(
        value["properties"]["master_ips"]["x-keel"],
        json!({ "check": "ip" })
    );
    assert_eq!(
        value["properties"]["master_ips"]["x-ui"],
        json!({ "control": "tags" })
    );
    assert_eq!(
        value["properties"]["tsig_key"]["x-keel"],
        json!({ "secret": true })
    );
    assert_eq!(
        value["properties"]["tsig_key"]["x-ui"],
        json!({ "control": "password", "generate": true })
    );
    assert_eq!(
        value["properties"]["account"]["x-keel"],
        json!({ "level": "reseller", "options": "keel.account.options" })
    );
    assert_eq!(value["properties"]["account"].get("x-ui"), None);
    for plain in ["id", "kind", "ttl"] {
        assert_eq!(value["properties"][plain].get("x-keel"), None, "{plain}");
        assert_eq!(value["properties"][plain].get("x-ui"), None, "{plain}");
    }
}

#[test]
fn zone_edit_attributes_are_readable_through_the_typed_attribute_spaces() {
    let schema = schema_for!(ZoneEdit);

    let name = KeelAttrs::of(&property(&schema, "name"))
        .unwrap()
        .expect("x-keel on name");
    assert_eq!(
        name,
        KeelAttrs {
            check: Some(Check::Domain),
            ..Default::default()
        }
    );

    let master_ips = property(&schema, "master_ips");
    assert_eq!(
        KeelAttrs::of(&master_ips).unwrap().expect("x-keel").check,
        Some(Check::Ip)
    );
    assert_eq!(
        UiAttrs::of(&master_ips).unwrap().expect("x-ui").control,
        Some(Control::Tags)
    );

    let tsig_key = property(&schema, "tsig_key");
    assert!(KeelAttrs::of(&tsig_key).unwrap().expect("x-keel").secret);
    let ui = UiAttrs::of(&tsig_key).unwrap().expect("x-ui");
    assert_eq!(ui.control, Some(Control::Password));
    assert!(ui.generate);

    let account = KeelAttrs::of(&property(&schema, "account"))
        .unwrap()
        .expect("x-keel on account");
    assert_eq!(account.level, Some(Level::Reseller));
    assert_eq!(account.options.as_deref(), Some("keel.account.options"));

    assert_eq!(KeelAttrs::of(&property(&schema, "ttl")).unwrap(), None);
    assert_eq!(UiAttrs::of(&property(&schema, "ttl")).unwrap(), None);
}

#[test]
fn zone_edit_schema_keeps_the_identifier_as_a_string() {
    let schema = schema_for!(ZoneEdit);
    let value = schema.as_value();
    assert_eq!(value["$defs"]["Id"]["type"], json!("string"));
    assert_eq!(value["$defs"]["Id"]["minLength"], json!(26));
    let kind = &value["$defs"]["ZoneKind"];
    assert_eq!(kind["enum"], json!(["master", "slave"]));
}
