use std::collections::BTreeMap;

use keel_sdk::multipart::BASE64_ENCODING;
use keel_types::descriptor::KeelAttrs;
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub fields: BTreeMap<String, Node>,
    pub items: Option<Box<Tree>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub attrs: KeelAttrs,
    pub base64: bool,
    pub nested: Tree,
}

impl Tree {
    pub fn of(schema: &Value) -> Self {
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        build(schema, &defs, 0)
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.items.is_none()
    }
}

fn build(schema: &Value, defs: &Map<String, Value>, depth: usize) -> Tree {
    if depth > MAX_DEPTH {
        return Tree::default();
    }
    let Some(schema) = resolve(schema, defs) else {
        return Tree::default();
    };
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let fields = properties
            .iter()
            .map(|(name, property)| {
                let attrs = schemars::Schema::try_from(property.clone())
                    .ok()
                    .and_then(|property| KeelAttrs::of(&property).ok().flatten())
                    .unwrap_or_default();
                let node = Node {
                    attrs,
                    base64: property
                        .get("contentEncoding")
                        .and_then(Value::as_str)
                        .is_some_and(|encoding| encoding.eq_ignore_ascii_case(BASE64_ENCODING)),
                    nested: build(property, defs, depth + 1),
                };
                (name.clone(), node)
            })
            .collect();
        return Tree {
            fields,
            items: None,
        };
    }
    if let Some(items) = schema.get("items") {
        let nested = build(items, defs, depth + 1);
        return Tree {
            fields: BTreeMap::new(),
            items: (!nested.is_empty()).then(|| Box::new(nested)),
        };
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        for branch in schema
            .get(combinator)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let nested = build(branch, defs, depth + 1);
            if !nested.is_empty() {
                return nested;
            }
        }
    }
    Tree::default()
}

fn resolve<'a>(schema: &'a Value, defs: &'a Map<String, Value>) -> Option<&'a Value> {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => reference
            .strip_prefix("#/$defs/")
            .and_then(|name| defs.get(name)),
        None => Some(schema),
    }
}

pub fn push_segment(path: &mut String, name: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(name);
}

pub fn push_index(path: &mut String, index: usize) {
    path.push('[');
    path.push_str(&index.to_string());
    path.push(']');
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_types::{Check, Kind};

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Outer {
        #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
        name: String,
        records: Vec<Record>,
        primary: Option<Record>,
        #[schemars(extend("x-keel" = json!({ "level": "admin" })))]
        note: Option<String>,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Record {
        #[schemars(extend("x-keel" = json!({ "check": "ip" })))]
        ip: String,
        #[schemars(extend("x-keel" = json!({ "secret": true })))]
        key: Option<String>,
        #[schemars(extend("contentEncoding" = "base64", "x-keel" = json!({ "file": { "maxBytes": 16, "types": ["application/octet-stream"] } })))]
        blob: Option<String>,
    }

    #[test]
    fn the_tree_follows_defs_items_and_optional_branches_to_nested_attributes() {
        let descriptor = function::<Outer, Record>("t.outer.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<Outer>| async { unreachable!() })
            .descriptor()
            .unwrap();
        let tree = Tree::of(descriptor.params.as_value());
        assert_eq!(tree.fields["name"].attrs.check, Some(Check::Domain));
        assert_eq!(tree.fields["note"].attrs.level, Some(Level::Admin));
        let row = tree.fields["records"].nested.items.as_deref().unwrap();
        assert_eq!(row.fields["ip"].attrs.check, Some(Check::Ip));
        assert!(row.fields["key"].attrs.secret);
        assert!(!row.fields["key"].base64);
        assert!(row.fields["blob"].base64);
        assert_eq!(
            row.fields["blob"]
                .attrs
                .file
                .as_ref()
                .map(|file| file.max_bytes),
            Some(16)
        );
        let primary = &tree.fields["primary"].nested;
        assert_eq!(primary.fields["ip"].attrs.check, Some(Check::Ip));
        assert!(tree.fields["name"].nested.is_empty());
        assert!(Tree::of(&json!({ "type": "string" })).is_empty());
    }

    #[test]
    fn a_self_referencing_schema_stops_at_the_depth_cap() {
        let schema = json!({
            "$defs": { "Node": { "type": "object", "properties": { "child": { "$ref": "#/$defs/Node" } } } },
            "type": "object",
            "properties": { "root": { "$ref": "#/$defs/Node" } }
        });
        let mut depth = 0;
        let mut tree = &Tree::of(&schema);
        while let Some(node) = tree.fields.values().next() {
            depth += 1;
            tree = &node.nested;
        }
        assert_eq!(depth, MAX_DEPTH + 1);
    }

    #[test]
    fn path_segments_join_with_dots_and_brackets() {
        let mut path = String::new();
        push_segment(&mut path, "records");
        push_index(&mut path, 1);
        push_segment(&mut path, "ip");
        assert_eq!(path, "records[1].ip");
    }
}
