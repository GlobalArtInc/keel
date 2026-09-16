use std::collections::BTreeSet;

use keel_sdk::runtime::property_names;
use keel_sdk::{FunctionDef, KeelError};
use keel_types::{Descriptor, ErrorCode, FieldError, Kind, Level, Mode};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::schema::{push_index, push_segment};
use super::strip::Plan;

pub const IDS: &str = "ids";
pub const FIELDS: &str = "fields";
pub const MASK: &str = "mask";
pub const MULTI_FIELD_KEY: &str = "keel.error.multi_field";
pub const MULTI_UNMASKED_KEY: &str = "keel.error.multi_unmasked";
pub const GROUP_BODY_KEY: &str = "keel.error.group_body";

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub ids: Vec<Value>,
    pub fields: Map<String, Value>,
    pub mask: Vec<String>,
}

#[derive(Deserialize)]
struct Body {
    ids: Vec<Value>,
    #[serde(default)]
    fields: Map<String, Value>,
    #[serde(default)]
    mask: Vec<String>,
}

pub fn accepts_groups(def: &FunctionDef, descriptor: &Descriptor) -> bool {
    def.kind() == Kind::Form
        && def.mode() == Mode::Write
        && def.key().is_some()
        && descriptor
            .ui
            .as_ref()
            .is_some_and(|ui| !ui.multi.is_empty())
}

pub fn detect(
    def: &FunctionDef,
    descriptor: &Descriptor,
    params: &Value,
) -> Result<Option<Group>, KeelError> {
    if !accepts_groups(def, descriptor) {
        return Ok(None);
    }
    let Some(key) = def.key() else {
        return Ok(None);
    };
    let Value::Object(object) = params else {
        return Ok(None);
    };
    if !object.contains_key(IDS) || object.get(key).is_some_and(|value| !value.is_null()) {
        return Ok(None);
    }
    let body: Body = serde_json::from_value(params.clone()).map_err(|error| {
        KeelError::validation(GROUP_BODY_KEY)
            .param("function", def.name())
            .source(error)
    })?;
    if body.ids.iter().any(|id| !id.is_string()) {
        return Err(KeelError::validation(GROUP_BODY_KEY)
            .param("function", def.name())
            .field(IDS));
    }
    Ok(Some(Group {
        ids: body.ids,
        fields: body.fields,
        mask: body.mask,
    }))
}

pub fn check_mask(
    group: &Group,
    multi: &[String],
    plan: &Plan,
    level: Level,
) -> Result<(), KeelError> {
    let mut errors = Vec::new();
    let mut path = String::new();
    for (index, name) in group.mask.iter().enumerate() {
        if multi.contains(name) && plan.visible(name, level) {
            continue;
        }
        path.clear();
        push_segment(&mut path, MASK);
        push_index(&mut path, index);
        errors.push(field_error(&path, MULTI_FIELD_KEY, name));
    }
    for name in group.fields.keys() {
        if group.mask.contains(name) {
            continue;
        }
        path.clear();
        push_segment(&mut path, FIELDS);
        push_segment(&mut path, name);
        errors.push(field_error(&path, MULTI_UNMASKED_KEY, name));
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(KeelError::new(ErrorCode::Validation).fields(errors))
}

fn field_error(path: &str, key: &str, field: &str) -> FieldError {
    let mut params = Map::new();
    params.insert("field".to_owned(), Value::String(field.to_owned()));
    FieldError {
        path: path.to_owned(),
        code: Some(key.to_owned()),
        key: Some(key.to_owned()),
        params,
    }
}

pub fn keyed(key: &str, ids: &[Value]) -> Value {
    let mut params = Map::new();
    params.insert(key.to_owned(), Value::Array(ids.to_vec()));
    Value::Object(params)
}

pub fn merge(
    current: Value,
    properties: &BTreeSet<String>,
    plan: &Plan,
    key: &str,
    id: &Value,
    fields: &Map<String, Value>,
) -> Value {
    let mut merged = Map::new();
    if let Value::Object(object) = current {
        for (name, value) in object {
            if properties.contains(&name) && !plan.is_secret(&name) {
                merged.insert(name, value);
            }
        }
    }
    merged.insert(key.to_owned(), id.clone());
    for (name, value) in fields {
        merged.insert(name.clone(), value.clone());
    }
    Value::Object(merged)
}

pub fn properties(descriptor: &Descriptor) -> BTreeSet<String> {
    property_names(&descriptor.params)
}

#[cfg(test)]
mod tests {
    use keel_sdk::function;
    use keel_sdk::prelude::*;

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Edit {
        id: Option<Id>,
        title: String,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        note: Option<String>,
        pages: i32,
        #[schemars(extend("x-keel" = json!({ "secret": true })))]
        token: Option<String>,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Saved {
        id: Id,
    }

    fn form(multi: &[&str]) -> FunctionDef {
        function::<Edit, Saved>("t.book.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .key("id")
            .unowned()
            .ui(Form::new("t.book.edit.title")
                .read("t.book.get")
                .page("main", ["title", "note", "pages"])
                .multi(multi.iter().copied()))
            .handler(|_: Call<Edit>| async { unreachable!() })
    }

    fn id() -> Value {
        Value::String(Id::generate().to_string())
    }

    #[test]
    fn a_group_body_is_told_apart_by_ids_without_the_key_and_only_on_a_multi_form() {
        let def = form(&["pages", "note"]);
        let descriptor = def.descriptor().unwrap();
        let group = detect(
            &def,
            &descriptor,
            &json!({ "ids": [id(), id()], "fields": { "pages": 3 }, "mask": ["pages"] }),
        )
        .unwrap()
        .expect("a group body");
        assert_eq!(group.ids.len(), 2);
        assert_eq!(
            group.fields,
            json!({ "pages": 3 }).as_object().unwrap().clone()
        );
        assert_eq!(group.mask, ["pages"]);
        assert!(
            detect(
                &def,
                &descriptor,
                &json!({ "id": id(), "title": "x", "pages": 1 })
            )
            .unwrap()
            .is_none()
        );
        assert!(
            detect(&def, &descriptor, &json!({ "id": id(), "ids": [id()] }))
                .unwrap()
                .is_none()
        );
        assert!(
            detect(&def, &descriptor, &json!({ "title": "x", "pages": 1 }))
                .unwrap()
                .is_none()
        );
        assert!(detect(&def, &descriptor, &json!("bare")).unwrap().is_none());
        let plain = form(&[]);
        let plain_descriptor = plain.descriptor().unwrap();
        assert!(
            detect(&plain, &plain_descriptor, &json!({ "ids": [id()] }))
                .unwrap()
                .is_none()
        );
        let malformed = detect(&def, &descriptor, &json!({ "ids": "one" })).unwrap_err();
        assert_eq!(malformed.code(), ErrorCode::Validation);
        assert_eq!(malformed.key(), GROUP_BODY_KEY);
        let not_strings = detect(&def, &descriptor, &json!({ "ids": [1, 2] })).unwrap_err();
        assert_eq!(not_strings.field_path(), Some(IDS));
    }

    #[test]
    fn the_mask_must_sit_inside_the_visible_multi_and_the_fields_inside_the_mask() {
        let def = form(&["pages", "note"]);
        let descriptor = def.descriptor().unwrap();
        let plan = Plan::of_params(&descriptor);
        let multi = descriptor.ui.as_ref().unwrap().multi.clone();
        let group = Group {
            ids: vec![id()],
            fields: json!({ "pages": 3, "stray": 1 })
                .as_object()
                .unwrap()
                .clone(),
            mask: vec!["pages".into(), "title".into(), "note".into()],
        };
        let error = check_mask(&group, &multi, &plan, Level::User).unwrap_err();
        let wire = error.to_call_error();
        assert_eq!(wire.code, ErrorCode::Validation);
        let summary: Vec<(String, String, Value)> = wire
            .fields
            .iter()
            .map(|field| {
                (
                    field.path.clone(),
                    field.key.clone().unwrap(),
                    field.params["field"].clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "mask[1]".to_owned(),
                    MULTI_FIELD_KEY.to_owned(),
                    json!("title")
                ),
                (
                    "mask[2]".to_owned(),
                    MULTI_FIELD_KEY.to_owned(),
                    json!("note")
                ),
                (
                    "fields.stray".to_owned(),
                    MULTI_UNMASKED_KEY.to_owned(),
                    json!("stray")
                ),
            ]
        );
        let admin = Group {
            ids: vec![id()],
            fields: json!({ "note": "n" }).as_object().unwrap().clone(),
            mask: vec!["note".into(), "pages".into()],
        };
        check_mask(&admin, &multi, &plan, Level::Admin).unwrap();
        let empty = Group {
            ids: vec![id()],
            fields: Map::new(),
            mask: Vec::new(),
        };
        check_mask(&empty, &multi, &plan, Level::User).unwrap();
    }

    #[test]
    fn the_merge_keeps_the_schema_properties_of_the_read_object_and_lays_the_fields_over_it() {
        let def = form(&["pages"]);
        let descriptor = def.descriptor().unwrap();
        let properties = properties(&descriptor);
        let plan = Plan::of_params(&descriptor);
        let row = id();
        let merged = merge(
            json!({ "id": "old", "title": "book", "pages": 1, "token": "***", "accountId": "acc", "updatedAt": "t" }),
            &properties,
            &plan,
            "id",
            &row,
            json!({ "pages": 7 }).as_object().unwrap(),
        );
        assert_eq!(merged, json!({ "id": row, "title": "book", "pages": 7 }));
        assert_eq!(
            merge(json!(null), &properties, &plan, "id", &row, &Map::new()),
            json!({ "id": row })
        );
        assert_eq!(
            keyed("id", std::slice::from_ref(&row)),
            json!({ "id": [row] })
        );
    }
}
