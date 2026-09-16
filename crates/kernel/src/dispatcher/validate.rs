use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::paths::LocationSegment;
use jsonschema::{JsonType, ValidationError, Validator};
use keel_sdk::KeelError;
use keel_sdk::runtime::LIST_QUERY_REF;
use keel_types::{Check, Descriptor, ErrorCode, FieldError, FileAttrs, ListQuery};
use schemars::schema_for;
use serde_json::{Map, Value, json};

use super::schema::{Node, Tree, push_index, push_segment};

pub const FILE_TOO_LARGE_KEY: &str = "keel.error.file_too_large";
pub const FILE_TYPE_KEY: &str = "keel.error.file_type";
pub const FILE_ENCODING_KEY: &str = "keel.error.file_encoding";

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
        Self::compile(&descriptor.name, schema)
    }

    pub fn partial(descriptor: &Descriptor, names: &[String]) -> Result<Self, SchemaError> {
        let params = descriptor.params.as_value();
        let mut schema = Map::new();
        schema.insert("type".to_owned(), json!("object"));
        let properties: Map<String, Value> = params
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(name, _)| names.contains(name))
            .map(|(name, property)| (name.clone(), property.clone()))
            .collect();
        schema.insert("properties".to_owned(), Value::Object(properties));
        if let Some(defs) = params.get("$defs") {
            schema.insert("$defs".to_owned(), defs.clone());
        }
        Self::compile(&descriptor.name, &Value::Object(schema))
    }

    fn compile(function: &str, schema: &Value) -> Result<Self, SchemaError> {
        let validator = jsonschema::validator_for(schema).map_err(|error| SchemaError {
            function: function.to_owned(),
            reason: error.to_string(),
        })?;
        Ok(Self {
            schema: validator,
            checks: Tree::of(schema),
        })
    }

    pub fn validate(&self, params: &Value) -> Result<(), KeelError> {
        self.validate_at("", params)
    }

    pub fn validate_at(&self, prefix: &str, params: &Value) -> Result<(), KeelError> {
        let mut fields = Vec::new();
        for error in self.schema.iter_errors(params) {
            unfold(&error, prefix, &mut fields);
        }
        let mut path = prefix.to_owned();
        check(&self.checks, params, &mut path, &mut fields);
        if fields.is_empty() {
            return Ok(());
        }
        Err(KeelError::new(ErrorCode::Validation).fields(fields))
    }
}

pub fn file_too_large(path: &str, file: &FileAttrs, size: Option<u64>) -> FieldError {
    let mut params = Map::new();
    params.insert("limit".to_owned(), json!(file.max_bytes));
    if let Some(size) = size {
        params.insert("size".to_owned(), json!(size));
    }
    FieldError {
        path: path.to_owned(),
        code: Some(FILE_TOO_LARGE_KEY.to_owned()),
        key: Some(FILE_TOO_LARGE_KEY.to_owned()),
        params,
    }
}

pub fn file_type(path: &str, file: &FileAttrs, found: Option<&str>) -> FieldError {
    let mut params = Map::new();
    params.insert("types".to_owned(), json!(file.types));
    params.insert(
        "type".to_owned(),
        found.map_or(Value::Null, |found| Value::String(found.to_owned())),
    );
    FieldError {
        path: path.to_owned(),
        code: Some(FILE_TYPE_KEY.to_owned()),
        key: Some(FILE_TYPE_KEY.to_owned()),
        params,
    }
}

pub fn file_encoding(path: &str) -> FieldError {
    FieldError {
        path: path.to_owned(),
        code: Some(FILE_ENCODING_KEY.to_owned()),
        key: Some(FILE_ENCODING_KEY.to_owned()),
        params: Map::new(),
    }
}

fn inline_size(node: &Node, text: &str) -> u64 {
    if node.base64 {
        let trimmed = text.trim_end_matches('=');
        return match STANDARD.decode(text) {
            Ok(decoded) => decoded.len() as u64,
            Err(_) => (trimmed.len() as u64).saturating_mul(3) / 4,
        };
    }
    text.len() as u64
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
                if let (Some(file), Value::String(text)) = (&node.attrs.file, nested) {
                    let size = inline_size(node, text);
                    if size > file.max_bytes {
                        into.push(file_too_large(path, file, Some(size)));
                    }
                }
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

fn unfold(error: &ValidationError<'_>, prefix: &str, into: &mut Vec<FieldError>) {
    let context = match error.kind() {
        ValidationErrorKind::AnyOf { context } | ValidationErrorKind::OneOfNotValid { context } => {
            context
        }
        _ => {
            into.push(field_error(error, prefix));
            return;
        }
    };
    let mut branches = context.iter().filter(|errors| !expects_null(errors));
    match (branches.next(), branches.next()) {
        (Some(branch), None) => {
            for error in branch {
                unfold(error, prefix, into);
            }
        }
        _ => into.push(field_error(error, prefix)),
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

fn field_error(error: &ValidationError<'_>, prefix: &str) -> FieldError {
    let mut path = instance_path(error, prefix);
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

fn instance_path(error: &ValidationError<'_>, prefix: &str) -> String {
    let mut path = prefix.to_owned();
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

    #[derive(Deserialize, Serialize, JsonSchema)]
    struct Put {
        #[schemars(extend("x-keel" = json!({ "file": { "maxBytes": 8, "types": ["text/plain"] } }), "x-ui" = json!({ "control": "file" })))]
        text: String,
        #[schemars(extend("contentEncoding" = "base64", "x-keel" = json!({ "file": { "maxBytes": 4, "types": ["application/octet-stream"] } }), "x-ui" = json!({ "control": "file" })))]
        blob: Option<String>,
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
    fn a_partial_validator_checks_the_named_fields_alone_under_a_path_prefix() {
        let descriptor = function::<ZoneEdit, Nested>("t.zone.edit")
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .handler(|_: Call<ZoneEdit>| async { unreachable!() })
            .descriptor()
            .unwrap();
        let partial =
            ParamsValidator::partial(&descriptor, &["ttl".to_owned(), "master_ips".to_owned()])
                .unwrap();
        partial
            .validate_at("fields", &json!({ "ttl": 300 }))
            .unwrap();
        partial.validate_at("fields", &json!({})).unwrap();
        let wire = partial
            .validate_at(
                "fields",
                &json!({ "ttl": -1, "master_ips": ["10.0.0"], "name": 7 }),
            )
            .unwrap_err()
            .to_call_error();
        let summary: Vec<(String, String)> = wire
            .fields
            .iter()
            .map(|field| (field.path.clone(), field.code.clone().unwrap()))
            .collect();
        assert_eq!(
            summary,
            [
                ("fields.ttl".to_owned(), "schema.minimum".to_owned()),
                ("fields.master_ips[0]".to_owned(), "check.ip".to_owned()),
            ]
        );
    }

    #[test]
    fn an_inline_file_value_is_measured_against_max_bytes_decoded_when_base64() {
        let descriptor = function::<Put, Nested>("t.blob.put")
            .level(Level::User)
            .unowned()
            .upload(|_: Call<Put>| async { unreachable!() })
            .descriptor()
            .unwrap();
        let validator = ParamsValidator::new(&descriptor).unwrap();
        validator
            .validate(&json!({ "text": "12345678", "blob": "AAAAAA==" }))
            .unwrap();
        let wire = validator
            .validate(&json!({ "text": "123456789", "blob": "AAAAAAA=" }))
            .unwrap_err()
            .to_call_error();
        let summary: Vec<(String, String, Value, Value)> = wire
            .fields
            .iter()
            .map(|field| {
                (
                    field.path.clone(),
                    field.key.clone().unwrap(),
                    field.params["limit"].clone(),
                    field.params["size"].clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "text".to_owned(),
                    FILE_TOO_LARGE_KEY.to_owned(),
                    json!(8),
                    json!(9)
                ),
                (
                    "blob".to_owned(),
                    FILE_TOO_LARGE_KEY.to_owned(),
                    json!(4),
                    json!(5)
                ),
            ]
        );
        let malformed = validator
            .validate(&json!({ "text": "ok", "blob": "not base64 at all!!" }))
            .unwrap_err()
            .to_call_error();
        assert_eq!(malformed.fields[0].path, "blob");
        let file = FileAttrs::new(8, ["text/plain"]);
        let refused = file_type("text", &file, Some("image/png"));
        assert_eq!(refused.key.as_deref(), Some(FILE_TYPE_KEY));
        assert_eq!(refused.params["type"], json!("image/png"));
        assert_eq!(refused.params["types"], json!(["text/plain"]));
        assert_eq!(file_type("text", &file, None).params["type"], Value::Null);
        assert_eq!(
            file_encoding("text").key.as_deref(),
            Some(FILE_ENCODING_KEY)
        );
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
