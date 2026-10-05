use keel_types::Id;
use serde_json::{Map, Value, json};

use crate::report::{Failure, fail};

const DEPTH: usize = 16;
const ID_PATTERN: &str = "^[0-7][0-9A-HJKMNP-TV-Z]{25}$";
const NUMBERS: [&str; 2] = ["integer", "number"];
const MOST: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Least,
    Next,
}

pub fn instance(schema: &Value, pick: Pick) -> Result<Value, Failure> {
    synthesise(schema, schema, pick, 0).map_err(|reason| {
        fail(format!(
            "no value can be built for the schema {schema}: {reason}"
        ))
    })
}

fn resolve<'a>(root: &'a Value, reference: &str) -> Result<&'a Value, String> {
    let pointer = reference
        .strip_prefix('#')
        .ok_or_else(|| format!("the reference {reference} leaves the document"))?;
    root.pointer(pointer)
        .ok_or_else(|| format!("the reference {reference} names nothing"))
}

fn synthesise(root: &Value, schema: &Value, pick: Pick, depth: usize) -> Result<Value, String> {
    if depth > DEPTH {
        return Err(format!("the schema nests deeper than {DEPTH}"));
    }
    let object = match schema {
        Value::Bool(true) => return Ok(Value::Null),
        Value::Bool(false) => return Err("the schema false admits nothing".to_owned()),
        Value::Object(object) => object,
        other => return Err(format!("{other} is not a schema")),
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return synthesise(root, resolve(root, reference)?, pick, depth + 1);
    }
    if let Some(constant) = object.get("const") {
        return Ok(constant.clone());
    }
    if let Some(first) = object
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Ok(first.clone());
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = object.get(combinator).and_then(Value::as_array) {
            let mut reasons = Vec::new();
            for branch in branches {
                match synthesise(root, branch, pick, depth + 1) {
                    Ok(value) => return Ok(value),
                    Err(reason) => reasons.push(reason),
                }
            }
            return Err(format!(
                "no branch of {combinator} fits: {}",
                reasons.join("; ")
            ));
        }
    }
    let kind = match object.get("type") {
        Some(Value::String(kind)) => kind.as_str(),
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null")
            .unwrap_or("null"),
        Some(other) => return Err(format!("the type {other} is not a name")),
        None if object.contains_key("properties") => "object",
        None => return Err("the schema names no type".to_owned()),
    };
    match kind {
        "null" => Ok(Value::Null),
        "boolean" => Ok(Value::Bool(pick == Pick::Next)),
        name if NUMBERS.contains(&name) => number(object, pick),
        "string" => string(object, pick),
        "array" => array(root, object, pick, depth),
        "object" => properties(root, object, pick, depth),
        other => Err(format!("the type {other} is not known")),
    }
}

fn bound(
    object: &Map<String, Value>,
    inclusive: &str,
    exclusive: &str,
    step: i64,
) -> Result<Option<i64>, String> {
    if let Some(bound) = object.get(inclusive).and_then(Value::as_i64) {
        return Ok(Some(bound));
    }
    object
        .get(exclusive)
        .and_then(Value::as_i64)
        .map(|bound| {
            bound
                .checked_add(step)
                .ok_or_else(|| format!("no integer lies past the {exclusive} {bound}"))
        })
        .transpose()
}

fn number(object: &Map<String, Value>, pick: Pick) -> Result<Value, String> {
    let minimum = bound(object, "minimum", "exclusiveMinimum", 1)?;
    let maximum = bound(object, "maximum", "exclusiveMaximum", -1)?;
    let least = match (minimum, maximum) {
        (Some(minimum), _) => minimum,
        (None, Some(maximum)) if maximum < 0 => maximum,
        (None, _) => 0,
    };
    if maximum.is_some_and(|maximum| maximum < least) {
        return Err(format!("no number lies between {least} and {maximum:?}"));
    }
    let value = match pick {
        Pick::Least => least,
        Pick::Next => least
            .checked_add(1)
            .filter(|next| maximum.is_none_or(|maximum| *next <= maximum))
            .unwrap_or(least),
    };
    Ok(json!(value))
}

fn string(object: &Map<String, Value>, pick: Pick) -> Result<Value, String> {
    if let Some(pattern) = object.get("pattern").and_then(Value::as_str) {
        return if pattern == ID_PATTERN {
            Ok(json!(Id::generate()))
        } else {
            Err(format!("a string of the pattern {pattern} cannot be built"))
        };
    }
    let length = size(object, "minLength")?.max(1);
    let letter = match pick {
        Pick::Least => "a",
        Pick::Next => "b",
    };
    Ok(json!(letter.repeat(length)))
}

fn array(
    root: &Value,
    object: &Map<String, Value>,
    pick: Pick,
    depth: usize,
) -> Result<Value, String> {
    let count = size(object, "minItems")?;
    if count == 0 {
        return Ok(json!([]));
    }
    let items = object
        .get("items")
        .ok_or_else(|| "an array that needs items names no items".to_owned())?;
    let item = synthesise(root, items, pick, depth + 1)?;
    Ok(Value::Array(vec![item; count]))
}

fn size(object: &Map<String, Value>, keyword: &str) -> Result<usize, String> {
    let Some(value) = object.get(keyword) else {
        return Ok(0);
    };
    let size = value
        .as_u64()
        .ok_or_else(|| format!("{keyword} {value} is not a count"))?;
    if size > MOST {
        return Err(format!(
            "{keyword} {size} is above the {MOST} the suite builds"
        ));
    }
    usize::try_from(size).map_err(|_| format!("{keyword} {size} is not a count"))
}

fn properties(
    root: &Value,
    object: &Map<String, Value>,
    pick: Pick,
    depth: usize,
) -> Result<Value, String> {
    let mut built = Map::new();
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for name in required.iter().filter_map(Value::as_str) {
        let property = object
            .get("properties")
            .and_then(|properties| properties.get(name))
            .ok_or_else(|| format!("the required member {name} has no schema"))?;
        let value = synthesise(root, property, pick, depth + 1)
            .map_err(|reason| format!("{name}: {reason}"))?;
        built.insert(name.to_owned(), value);
    }
    Ok(Value::Object(built))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zones() -> Value {
        json!({
            "type": "object",
            "properties": {
                "zoneLimit": { "type": ["integer", "null"], "minimum": 0, "maximum": 100000 },
                "template": {
                    "anyOf": [{ "$ref": "#/$defs/Id" }, { "type": "null" }],
                    "default": null
                }
            },
            "required": ["zoneLimit"],
            "$defs": { "Id": { "type": "string", "pattern": ID_PATTERN } }
        })
    }

    #[test]
    fn the_least_instance_fills_the_required_members_with_their_lowest_values_and_the_next_moves_them()
     {
        assert_eq!(
            instance(&zones(), Pick::Least).unwrap(),
            json!({ "zoneLimit": 0 })
        );
        assert_eq!(
            instance(&zones(), Pick::Next).unwrap(),
            json!({ "zoneLimit": 1 })
        );
        let sites = json!({
            "type": "object",
            "properties": { "siteLimit": { "type": ["integer", "null"], "minimum": 0, "maximum": 0 } },
            "required": ["siteLimit"]
        });
        assert_eq!(
            instance(&sites, Pick::Next).unwrap(),
            json!({ "siteLimit": 0 })
        );
        assert_eq!(
            instance(&json!({ "type": "object" }), Pick::Least).unwrap(),
            json!({})
        );
    }

    #[test]
    fn references_combinators_enums_and_every_type_are_followed() {
        let schema = json!({
            "type": "object",
            "required": ["id", "flag", "mode", "tags", "inner", "fixed", "name", "ratio", "nothing"],
            "properties": {
                "id": { "$ref": "#/$defs/Id" },
                "flag": { "type": "boolean" },
                "mode": { "type": "string", "enum": ["slow", "fast"] },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 2 },
                "inner": { "properties": { "depth": { "type": "integer", "exclusiveMinimum": 3 } }, "required": ["depth"] },
                "fixed": { "const": 7 },
                "name": { "type": "string", "minLength": 3 },
                "ratio": { "type": "number", "maximum": -2 },
                "nothing": { "type": "null" }
            },
            "$defs": { "Id": { "type": "string", "pattern": ID_PATTERN } }
        });
        let built = instance(&schema, Pick::Least).unwrap();
        assert!(built["id"].as_str().unwrap().parse::<Id>().is_ok());
        assert_eq!(built["flag"], json!(false));
        assert_eq!(built["mode"], json!("slow"));
        assert_eq!(built["tags"], json!(["a", "a"]));
        assert_eq!(built["inner"], json!({ "depth": 4 }));
        assert_eq!(built["fixed"], json!(7));
        assert_eq!(built["name"], json!("aaa"));
        assert_eq!(built["ratio"], json!(-2));
        assert_eq!(built["nothing"], Value::Null);
        let next = instance(&schema, Pick::Next).unwrap();
        assert_eq!(next["flag"], json!(true));
        assert_eq!(next["name"], json!("bbb"));
        assert_eq!(next["inner"], json!({ "depth": 5 }));
    }

    #[test]
    fn the_ends_of_the_integer_range_and_the_largest_sizes_build_without_overflow() {
        let top = json!({ "type": "integer", "minimum": i64::MAX });
        assert_eq!(instance(&top, Pick::Least).unwrap(), json!(i64::MAX));
        assert_eq!(instance(&top, Pick::Next).unwrap(), json!(i64::MAX));
        let bottom = json!({ "type": "integer", "maximum": i64::MIN });
        assert_eq!(instance(&bottom, Pick::Next).unwrap(), json!(i64::MIN));
        let edge = json!({ "type": "integer", "exclusiveMaximum": i64::MIN + 1 });
        assert_eq!(instance(&edge, Pick::Least).unwrap(), json!(i64::MIN));
        let long = json!({ "type": "string", "minLength": MOST });
        assert_eq!(
            instance(&long, Pick::Least).unwrap().as_str().map(str::len),
            Some(1024)
        );
        let many = json!({ "type": "array", "items": { "type": "null" }, "minItems": MOST });
        assert_eq!(
            instance(&many, Pick::Least)
                .unwrap()
                .as_array()
                .map(Vec::len),
            Some(1024)
        );
    }

    #[test]
    fn a_schema_no_value_can_satisfy_is_refused_with_its_reason() {
        for (schema, reason) in [
            (json!(false), "admits nothing"),
            (
                json!({ "type": "string", "pattern": "^x+$" }),
                "pattern ^x+$",
            ),
            (json!({ "$ref": "#/$defs/Missing" }), "names nothing"),
            (json!({ "$ref": "other.json#/a" }), "leaves the document"),
            (
                json!({ "type": "integer", "minimum": 5, "maximum": 1 }),
                "no number",
            ),
            (
                json!({ "type": "object", "required": ["a"], "properties": {} }),
                "member a has no schema",
            ),
            (
                json!({ "anyOf": [false, { "type": "string", "pattern": "^y$" }] }),
                "no branch",
            ),
            (json!({ "description": "untyped" }), "names no type"),
            (
                json!({ "type": "integer", "exclusiveMinimum": i64::MAX }),
                "past the exclusiveMinimum",
            ),
            (
                json!({ "type": "integer", "exclusiveMaximum": i64::MIN }),
                "past the exclusiveMaximum",
            ),
            (
                json!({ "type": "string", "minLength": u64::MAX }),
                "minLength 18446744073709551615 is above the 1024",
            ),
            (
                json!({ "type": "string", "minLength": MOST + 1 }),
                "above the 1024",
            ),
            (
                json!({ "type": "array", "items": { "type": "string" }, "minItems": u64::MAX }),
                "minItems 18446744073709551615 is above",
            ),
            (json!({ "type": "string", "minLength": -1 }), "not a count"),
        ] {
            let refused = instance(&schema, Pick::Least).unwrap_err().0;
            assert!(refused.contains(reason), "{schema}: {refused}");
        }
        let mut deep = json!({ "type": "integer" });
        for _ in 0..=DEPTH {
            deep = json!({ "anyOf": [deep] });
        }
        assert!(
            instance(&deep, Pick::Least)
                .unwrap_err()
                .0
                .contains("deeper")
        );
    }
}
