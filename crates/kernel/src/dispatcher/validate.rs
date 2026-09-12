use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::paths::LocationSegment;
use jsonschema::{JsonType, ValidationError, Validator};
use keel_sdk::KeelError;
use keel_sdk::runtime::LIST_QUERY_REF;
use keel_types::{Check, Descriptor, ErrorCode, FieldError, ListQuery};
use schemars::schema_for;
use serde_json::{Map, Value, json};

use super::schema::{Tree, push_index, push_segment};

pub struct ParamsValidator {
    schema: Validator,
    checks: Tree,
}

#[derive(Debug, thiserror::Error)]
#[error("function {function}: the parameter schema does not compile: {reason}")]
pub struct SchemaError {
    pub function: String,
    pub reason: String,
}

impl ParamsValidator {
    pub fn new(descriptor: &Descriptor) -> Result<Self, SchemaError> {
        let params = descriptor.params.as_value();
        let list_query;
        let schema = if params.get("$ref").and_then(Value::as_str) == Some(LIST_QUERY_REF) {
            list_query =
                serde_json::to_value(schema_for!(ListQuery)).map_err(|error| SchemaError {
                    function: descriptor.name.clone(),
                    reason: error.to_string(),
                })?;
            &list_query
        } else {
            params
        };
        let validator = jsonschema::validator_for(schema).map_err(|error| SchemaError {
            function: descriptor.name.clone(),
            reason: error.to_string(),
        })?;
        Ok(Self {
            schema: validator,
            checks: Tree::of(schema),
        })
    }

    pub fn validate(&self, params: &Value) -> Result<(), KeelError> {
        let mut fields = Vec::new();
        for error in self.schema.iter_errors(params) {
            unfold(&error, &mut fields);
        }
        check(&self.checks, params, &mut String::new(), &mut fields);
        if fields.is_empty() {
            return Ok(());
        }
        Err(KeelError::new(ErrorCode::Validation).fields(fields))
    }
}

fn check(tree: &Tree, value: &Value, path: &mut String, into: &mut Vec<FieldError>) {
    match value {
        Value::Object(object) => {
            for (name, nested) in object {
                let Some(node) = tree.fields.get(name) else {
                    continue;
                };
                let start = path.len();
                push_segment(path, name);
                if let Some(preset) = node.attrs.check {
                    match nested {
                        Value::String(text) => probe(preset, text, path, into),
                        Value::Array(items) => {
                            for (index, item) in items.iter().enumerate() {
                                if let Value::String(text) = item {
                                    let start = path.len();
                                    push_index(path, index);
                                    probe(preset, text, path, into);
                                    path.truncate(start);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                check(&node.nested, nested, path, into);
                path.truncate(start);
            }
        }
        Value::Array(items) => {
            if let Some(nested) = &tree.items {
                for (index, item) in items.iter().enumerate() {
                    let start = path.len();
                    push_index(path, index);
                    check(nested, item, path, into);
                    path.truncate(start);
                }
            }
        }
        _ => {}
    }
}

fn probe(preset: Check, text: &str, path: &str, into: &mut Vec<FieldError>) {
    if preset.matches(text) {
        return;
    }
    let mut params = Map::new();
    params.insert("value".to_owned(), Value::String(text.to_owned()));
    into.push(FieldError {
        path: path.to_owned(),
        code: Some(preset.code()),
        key: Some(preset.key()),
        params,
    });
}

fn unfold(error: &ValidationError<'_>, into: &mut Vec<FieldError>) {
    let context = match error.kind() {
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            context
        }
        _ => {
            into.push(field_error(error));
            return;
        }
    };
    let mut branches = context.iter().filter(|errors| !expects_null(errors));
    match (branches.next(), branches.next()) {
        (Some(branch), None) => {
            for error in branch {
                unfold(error, into);
            }
        }
        _ => into.push(field_error(error)),
    }
}

fn expects_null(errors: &[ValidationError<'_>]) -> bool {
    matches!(
        errors,
        [only] if matches!(
            only.kind(),
            ValidationErrorKind::Type { kind: TypeKind::Single(JsonType::Null) }
        )
    )
}

fn field_error(error: &ValidationError<'_>) -> FieldError {
    let mut path = instance_path(error);
    let mut params = Map::new();
    match error.kind() {
        ValidationErrorKind::Required { property } => {
            if let Some(property) = property.as_str() {
                push_segment(&mut path, property);
            }
            params.insert("property".to_owned(), property.clone());
        }
        ValidationErrorKind::Type { .. } | ValidationErrorKind::Enum { .. } => {
            params.insert("value".to_owned(), error.instance().as_ref().clone());
        }
        ValidationErrorKind::Minimum { limit }
        | ValidationErrorKind::Maximum { limit }
        | ValidationErrorKind::ExclusiveMinimum { limit }
        | ValidationErrorKind::ExclusiveMaximum { limit } => {
            params.insert("limit".to_owned(), limit.clone());
        }
        ValidationErrorKind::MinLength { limit }
        | ValidationErrorKind::MaxLength { limit }
        | ValidationErrorKind::MinItems { limit }
        | ValidationErrorKind::MaxItems { limit }
        | ValidationErrorKind::MinProperties { limit }
        | ValidationErrorKind::MaxProperties { limit } => {
            params.insert("limit".to_owned(), json!(limit));
        }
        _ => {}
    }
    let keyword = error
        .schema_path()
        .segments()
        .filter_map(|segment| match segment {
            LocationSegment::Property(name) => Some(name.into_owned()),
            LocationSegment::Index(_) => None,
        })
        .last()
        .unwrap_or_else(|| "schema".to_owned());
    FieldError {
        path,
        code: Some(format!("schema.{keyword}")),
        key: Some(format!("keel.schema.{keyword}")),
        params,
    }
}

fn instance_path(error: &ValidationError<'_>) -> String {
    let mut path = String::new();
    for segment in error.instance_path().segments() {
        match segment {
            LocationSegment::Property(name) => push_segment(&mut path, &name),
            LocationSegment::Index(index) => push_index(&mut path, index),
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_types::Kind;

    use super::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct ZoneEdit {
        id: Option<Id>,
        #[schemars(extend("x-keel" = json!({ "check": "domain" })))]
        name: String,
        #[schemars(extend("x-keel" = json!({ "check": "ip" })))]
        master_ips: Vec<String>,
        #[schemars(extend("x-keel" = json!({ "check": "port" })))]
        port: Option<String>,
        ttl: u32,
        nested: Option<Nested>,
        records: Option<Vec<Record>>,
        primary: Option<Record>,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Nested {
        depth: u8,
    }

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Record {
        #[schemars(extend("x-keel" = json!({ "check": "ip" })))]
        ip: String,
        #[schemars(extend("x-keel" = json!({ "check": "port" })))]
        ports: Vec<String>,
    }

    fn validator() -> ParamsValidator {
        let descriptor = function::<ZoneEdit, Nested>("t.zone.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<ZoneEdit>| async { unreachable!() })
            .descriptor()
            .unwrap();
        ParamsValidator::new(&descriptor).unwrap()
    }

    fn fields(params: Value) -> Vec<(String, String)> {
        validator()
            .validate(&params)
            .unwrap_err()
            .to_call_error()
            .fields
            .into_iter()
            .map(|field| (field.path, field.code.unwrap_or_default()))
            .collect()
    }

    #[test]
    fn schema_errors_carry_json_paths_and_schema_codes() {
        validator()
            .validate(&json!({ "name": "a.org", "master_ips": ["10.0.0.1"], "ttl": 300 }))
            .unwrap();
        assert_eq!(
            fields(json!({ "master_ips": [], "ttl": -1 })),
            [
                ("name".to_owned(), "schema.required".to_owned()),
                ("ttl".to_owned(), "schema.minimum".to_owned()),
            ]
        );
        assert_eq!(
            fields(
                json!({ "name": 7, "master_ips": ["1.1.1.1", 2], "ttl": 1, "nested": { "depth": "x" } })
            ),
            [
                ("name".to_owned(), "schema.type".to_owned()),
                ("master_ips[1]".to_owned(), "schema.type".to_owned()),
                ("nested.depth".to_owned(), "schema.type".to_owned()),
            ]
        );
        assert_eq!(
            fields(json!({ "name": "a.org", "master_ips": [], "ttl": 1, "nested": 4 })),
            [("nested".to_owned(), "schema.type".to_owned())]
        );
        let bad_id = fields(json!({ "id": "nope", "name": "a.org", "master_ips": [], "ttl": 1 }));
        assert_eq!(bad_id[0].0, "id");
        assert!(bad_id[0].1.starts_with("schema."), "{bad_id:?}");
    }

    #[test]
    fn check_presets_apply_to_strings_and_to_every_item_of_an_array() {
        let wire = validator()
            .validate(&json!({ "name": "not a domain", "master_ips": ["10.0.0.1", "10.0.0", "::1"], "port": "70000", "ttl": 1 }))
            .unwrap_err()
            .to_call_error();
        assert_eq!(wire.code, ErrorCode::Validation);
        assert_eq!(wire.status(), 422);
        let summary: Vec<(String, String, String)> = wire
            .fields
            .iter()
            .map(|field| {
                (
                    field.path.clone(),
                    field.code.clone().unwrap(),
                    field.key.clone().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "name".to_owned(),
                    "check.domain".to_owned(),
                    "keel.check.domain".to_owned()
                ),
                (
                    "master_ips[1]".to_owned(),
                    "check.ip".to_owned(),
                    "keel.check.ip".to_owned()
                ),
                (
                    "port".to_owned(),
                    "check.port".to_owned(),
                    "keel.check.port".to_owned()
                ),
            ]
        );
        assert_eq!(wire.fields[1].params["value"], json!("10.0.0"));
        validator()
            .validate(&json!({ "name": "a.org", "master_ips": [], "port": null, "ttl": 1 }))
            .unwrap();
    }

    #[test]
    fn check_presets_apply_below_the_top_level_with_the_full_json_path() {
        let wire = validator()
            .validate(&json!({
                "name": "a.org", "master_ips": [], "ttl": 1,
                "records": [
                    { "ip": "10.0.0.1", "ports": ["53"] },
                    { "ip": "10.0.0", "ports": ["53", "70000"] }
                ],
                "primary": { "ip": "::x", "ports": [] }
            }))
            .unwrap_err()
            .to_call_error();
        assert_eq!(wire.code, ErrorCode::Validation);
        let summary: Vec<(String, String)> = wire
            .fields
            .iter()
            .map(|field| (field.path.clone(), field.code.clone().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                ("records[1].ip".to_owned(), "check.ip".to_owned()),
                ("records[1].ports[1]".to_owned(), "check.port".to_owned()),
                ("primary.ip".to_owned(), "check.ip".to_owned()),
            ]
        );
        validator()
            .validate(&json!({
                "name": "a.org", "master_ips": [], "ttl": 1,
                "records": [ { "ip": "10.0.0.1", "ports": ["53"] } ],
                "primary": null
            }))
            .unwrap();
    }

    #[test]
    fn a_list_validates_against_the_shared_list_query() {
        let descriptor = function::<ListQuery, Page<Nested>>("t.zone.list")
            .kind(Kind::List)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<ListQuery>| async { unreachable!() })
            .descriptor()
            .unwrap();
        let validator = ParamsValidator::new(&descriptor).unwrap();
        validator.validate(&json!({})).unwrap();
        validator
            .validate(
                &json!({ "page": 2, "pageSize": 10, "sort": { "key": "name", "dir": "asc" } }),
            )
            .unwrap();
        let wire = validator
            .validate(&json!({ "page": "two", "sort": { "key": "name", "dir": "sideways" } }))
            .unwrap_err()
            .to_call_error();
        let paths: Vec<&str> = wire
            .fields
            .iter()
            .map(|field| field.path.as_str())
            .collect();
        assert_eq!(paths, ["page", "sort.dir"]);
    }
}
