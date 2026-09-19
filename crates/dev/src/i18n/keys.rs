use std::collections::BTreeSet;

use keel_types::{ColumnFilter, Descriptor, UiAttrs, X_UI};
use schemars::Schema;
use serde_json::{Map, Value};

use super::shared_of;

const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedKey {
    pub member: String,
    pub key: String,
    pub shared: Option<String>,
}

pub fn of(descriptor: &Descriptor) -> Vec<DerivedKey> {
    let Some(ui) = &descriptor.ui else {
        return Vec::new();
    };
    let mut keys = Keys {
        prefix: entity_prefix(&descriptor.name),
        seen: BTreeSet::new(),
        derived: Vec::new(),
    };
    let properties = properties(&descriptor.params);
    let defs = descriptor.params.get("$defs").and_then(Value::as_object);
    for field in fields(descriptor, properties) {
        keys.push(&format!("field {field}"), &field);
        let Some(schema) = properties.and_then(|properties| properties.get(&field)) else {
            continue;
        };
        for value in enum_values(schema, defs, 0) {
            keys.push(
                &format!("field {field} value {value}"),
                &format!("{field}.{value}"),
            );
        }
        if let Some(attrs) = schema
            .get(X_UI)
            .and_then(|value| serde_json::from_value::<UiAttrs>(value.clone()).ok())
        {
            if let Some(placeholder) = attrs.placeholder {
                keys.explicit(&format!("field {field} {X_UI}.placeholder"), &placeholder);
            }
            if let Some(help) = attrs.help {
                keys.explicit(&format!("field {field} {X_UI}.help"), &help);
            }
        }
    }
    for page in &ui.pages {
        keys.push(&format!("page {}", page.id), &format!("page.{}", page.id));
    }
    for action in &ui.actions {
        keys.push(
            &format!("action {}", action.id),
            &format!("action.{}", action.id),
        );
    }
    for column in &ui.columns {
        keys.push(
            &format!("column {}", column.key),
            &format!("column.{}", column.key),
        );
        let mut values: Vec<&str> = Vec::new();
        if let Some(ColumnFilter::Options { options }) = &column.filter {
            values.extend(options.iter().map(String::as_str));
        }
        if let Some(map) = &column.map {
            values.extend(map.keys().map(String::as_str));
        }
        for value in values {
            keys.push(
                &format!("column {} value {value}", column.key),
                &format!("{}.{value}", column.key),
            );
        }
    }
    keys.derived
}

struct Keys {
    prefix: String,
    seen: BTreeSet<String>,
    derived: Vec<DerivedKey>,
}

impl Keys {
    fn push(&mut self, member: &str, tail: &str) {
        let key = format!("{}.{tail}", self.prefix);
        if !self.seen.insert(key.clone()) {
            return;
        }
        let shared = shared_of(&key, tail);
        self.derived.push(DerivedKey {
            member: member.to_owned(),
            key,
            shared,
        });
    }

    fn explicit(&mut self, member: &str, key: &str) {
        if !self.seen.insert(key.to_owned()) {
            return;
        }
        self.derived.push(DerivedKey {
            member: member.to_owned(),
            key: key.to_owned(),
            shared: None,
        });
    }
}

fn entity_prefix(function: &str) -> String {
    function
        .rsplit_once('.')
        .map_or(function, |(prefix, _)| prefix)
        .to_owned()
}

fn properties(schema: &Schema) -> Option<&Map<String, Value>> {
    schema.get("properties").and_then(Value::as_object)
}

fn fields(descriptor: &Descriptor, properties: Option<&Map<String, Value>>) -> Vec<String> {
    let paged: Vec<String> = descriptor
        .ui
        .as_ref()
        .map(|ui| {
            ui.pages
                .iter()
                .flat_map(|page| page.fields.iter().cloned())
                .collect()
        })
        .unwrap_or_default();
    let from_pages = !paged.is_empty();
    let candidates = if from_pages {
        paged
    } else {
        properties
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut fields = Vec::new();
    for field in candidates {
        if !from_pages && descriptor.key.as_deref() == Some(field.as_str()) {
            continue;
        }
        if !fields.contains(&field) {
            fields.push(field);
        }
    }
    fields
}

fn enum_values(schema: &Value, defs: Option<&Map<String, Value>>, depth: usize) -> Vec<String> {
    let mut values = Vec::new();
    if depth > MAX_DEPTH {
        return values;
    }
    let Some(object) = schema.as_object() else {
        return values;
    };
    if let Some(Value::Array(listed)) = object.get("enum") {
        values.extend(listed.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str)
        && let Some(name) = reference.strip_prefix("#/$defs/")
        && let Some(target) = defs.and_then(|defs| defs.get(name))
    {
        values.extend(enum_values(target, defs, depth + 1));
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(branches)) = object.get(key) {
            for branch in branches {
                values.extend(enum_values(branch, defs, depth + 1));
            }
        }
    }
    if let Some(items) = object.get("items") {
        values.extend(enum_values(items, defs, depth + 1));
    }
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}
