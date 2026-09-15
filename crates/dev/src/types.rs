use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use keel_types::{
    Account, ActionRole, Boot, CallError, Check, Choices, ColumnType, Context, Control, Descriptor,
    Envelope, ErrorCode, Event, Expr, KeelAttrs, Kind, Level, ListQuery, Mode, NotificationFrame,
    OptionsQuery, Page, Principal, ScreenResult, ScreenUi, TaskProgress, TaskRef, TaskState, Tone,
    Total, UiAttrs,
};
use schemars::generate::SchemaSettings;
use schemars::transform::{Transform, transform_subschemas};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde_json::{Map, Value};

pub const DOCUMENT: &str = "keel.json";
const META_SCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";
const DEFINITIONS: &str = "#/$defs/";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{0} is inlined by its schema and cannot be a root type")]
    Inlined(String),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error(
        "{} would change: {}; run `keel-dev types --out <dir>` and commit the result",
        if .0.len() == 1 { "one file" } else { "files" },
        .0.join(", ")
    )]
    Changed(Vec<String>),
}

#[derive(Debug)]
pub struct Outcome {
    pub dir: PathBuf,
    pub files: Vec<String>,
    pub checked: bool,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.checked {
            "up to date in"
        } else {
            "written to"
        };
        write!(
            f,
            "{} schema files {verb} {}",
            self.files.len(),
            self.dir.display()
        )
    }
}

pub struct Document {
    pub schema: Value,
    pub roots: Vec<String>,
}

#[derive(Clone)]
struct Rename {
    from: String,
    to: String,
}

impl Rename {
    fn of<T: JsonSchema>(to: &str) -> Self {
        Self {
            from: T::schema_name().into_owned(),
            to: to.to_owned(),
        }
    }

    fn reference(name: &str) -> String {
        format!("{DEFINITIONS}{name}")
    }
}

impl Transform for Rename {
    fn transform(&mut self, schema: &mut Schema) {
        if let Some(definitions) = schema.get_mut("$defs").and_then(Value::as_object_mut)
            && let Some(definition) = definitions.remove(&self.from)
        {
            definitions.insert(self.to.clone(), definition);
        }
        if let Some(Value::String(reference)) = schema.get_mut("$ref")
            && *reference == Self::reference(&self.from)
        {
            *reference = Self::reference(&self.to);
        }
        transform_subschemas(self, schema);
    }
}

fn root<T: JsonSchema>(generator: &mut SchemaGenerator) -> Result<Schema, Error> {
    let schema = generator.subschema_for::<T>();
    if schema.get("$ref").is_none() {
        return Err(Error::Inlined(T::schema_name().into_owned()));
    }
    Ok(schema)
}

fn name_of(reference: &Schema) -> Result<String, Error> {
    reference
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|target| target.strip_prefix(DEFINITIONS))
        .map(str::to_owned)
        .ok_or_else(|| Error::Inlined(reference.as_value().to_string()))
}

pub fn document() -> Result<Document, Error> {
    let mut generator = SchemaSettings::draft2020_12()
        .with_transform(Rename::of::<Envelope<Value>>("Envelope"))
        .with_transform(Rename::of::<Page<Value>>("Page"))
        .into_generator();
    let mut roots = vec![
        root::<Descriptor>(&mut generator)?,
        root::<ScreenUi>(&mut generator)?,
        root::<KeelAttrs>(&mut generator)?,
        root::<UiAttrs>(&mut generator)?,
        root::<Envelope<Value>>(&mut generator)?,
        root::<CallError>(&mut generator)?,
        root::<ListQuery>(&mut generator)?,
        root::<Page<Value>>(&mut generator)?,
        root::<OptionsQuery>(&mut generator)?,
        root::<Choices>(&mut generator)?,
        root::<Expr>(&mut generator)?,
        root::<Context>(&mut generator)?,
        root::<Boot>(&mut generator)?,
        root::<Principal>(&mut generator)?,
        root::<Account>(&mut generator)?,
        root::<Event>(&mut generator)?,
        root::<Kind>(&mut generator)?,
        root::<Mode>(&mut generator)?,
        root::<Control>(&mut generator)?,
        root::<ColumnType>(&mut generator)?,
        root::<ActionRole>(&mut generator)?,
        root::<Check>(&mut generator)?,
        root::<ErrorCode>(&mut generator)?,
        root::<Level>(&mut generator)?,
        root::<Tone>(&mut generator)?,
        root::<Total>(&mut generator)?,
        root::<ScreenResult>(&mut generator)?,
        root::<TaskProgress>(&mut generator)?,
        root::<TaskRef>(&mut generator)?,
        root::<TaskState>(&mut generator)?,
        root::<NotificationFrame>(&mut generator)?,
    ];
    let definitions = generator.take_definitions(false);
    let mut schema = json_schema!({
        "$schema": META_SCHEMA,
        "$id": DOCUMENT,
        "$defs": definitions
    });
    for transform in generator.transforms_mut() {
        transform.transform(&mut schema);
        for reference in &mut roots {
            transform.transform(reference);
        }
    }
    let sorted: BTreeMap<String, Value> = schema
        .remove("$defs")
        .and_then(|definitions| match definitions {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .collect();
    schema.insert(
        "$defs".to_owned(),
        Value::Object(sorted.into_iter().collect::<Map<String, Value>>()),
    );
    let roots = roots.iter().map(name_of).collect::<Result<_, _>>()?;
    Ok(Document {
        schema: schema.to_value(),
        roots,
    })
}

pub fn files() -> Result<BTreeMap<String, String>, Error> {
    let document = document()?;
    let mut files = BTreeMap::new();
    files.insert(DOCUMENT.to_owned(), pretty(&document.schema)?);
    for name in &document.roots {
        let reference = serde_json::json!({ "$ref": format!("{DOCUMENT}{DEFINITIONS}{name}") });
        files.insert(format!("{name}.json"), pretty(&reference)?);
    }
    Ok(files)
}

fn pretty(value: &Value) -> Result<String, Error> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

fn io(path: &Path) -> impl FnOnce(io::Error) -> Error {
    let path = path.to_path_buf();
    move |source| Error::Io { path, source }
}

fn stale(out: &Path, files: &BTreeMap<String, String>) -> Result<Vec<String>, Error> {
    let entries = match fs::read_dir(out) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io(out)(source)),
    };
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io(out))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && !files.contains_key(&name) {
            stale.push(name);
        }
    }
    stale.sort();
    Ok(stale)
}

pub fn run(out: &Path, check: bool) -> Result<Outcome, Error> {
    let files = files()?;
    if check {
        let mut changed = Vec::new();
        for (name, content) in &files {
            let path = out.join(name);
            match fs::read_to_string(&path) {
                Ok(existing) if existing == *content => {}
                Ok(_) => changed.push(name.clone()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    changed.push(name.clone());
                }
                Err(source) => return Err(io(&path)(source)),
            }
        }
        changed.extend(stale(out, &files)?);
        if !changed.is_empty() {
            return Err(Error::Changed(changed));
        }
    } else {
        fs::create_dir_all(out).map_err(io(out))?;
        for (name, content) in &files {
            let path = out.join(name);
            fs::write(&path, content).map_err(io(&path))?;
        }
        for name in stale(out, &files)? {
            let path = out.join(name);
            fs::remove_file(&path).map_err(io(&path))?;
        }
    }
    Ok(Outcome {
        dir: out.to_path_buf(),
        files: files.into_keys().collect(),
        checked: check,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn committed() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema")
    }

    fn definitions() -> Map<String, Value> {
        document().unwrap().schema["$defs"]
            .as_object()
            .unwrap()
            .clone()
    }

    fn enum_values(definition: &Value) -> Vec<&str> {
        definition["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect()
    }

    #[test]
    fn the_document_is_draft_2020_12_named_keel_json_with_every_root_under_defs() {
        let document = document().unwrap();
        assert_eq!(document.schema["$schema"], json!(META_SCHEMA));
        assert_eq!(document.schema["$id"], json!(DOCUMENT));
        let definitions = document.schema["$defs"].as_object().unwrap();
        assert_eq!(document.roots.len(), 31);
        for root in &document.roots {
            assert!(definitions.contains_key(root), "{root} is under $defs");
        }
        let names: Vec<&String> = definitions.keys().collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "$defs are sorted by name");
    }

    #[test]
    fn kind_enumerates_the_ten_kinds_and_check_the_eight_presets() {
        let definitions = definitions();
        assert_eq!(
            enum_values(&definitions["Kind"]),
            [
                "list", "get", "form", "action", "wizard", "report", "options", "stream",
                "download", "upload"
            ]
        );
        assert_eq!(
            enum_values(&definitions["Check"]),
            [
                "domain", "ip", "cidr", "email", "int", "name", "port", "path"
            ]
        );
        assert_eq!(enum_values(&definitions["ErrorCode"]).len(), 13);
        assert_eq!(enum_values(&definitions["Level"]).len(), 6);
    }

    #[test]
    fn expr_refers_to_itself_and_a_descriptor_admits_an_object_or_a_boolean_schema() {
        let definitions = definitions();
        let expr = definitions["Expr"].to_string();
        assert!(expr.contains("\"$ref\":\"#/$defs/Expr\""));
        let branches = definitions["Expr"]["oneOf"].as_array().unwrap();
        assert_eq!(branches.len(), 11);
        assert_eq!(
            branches[0]["properties"]["eq"]["prefixItems"],
            json!([{ "type": "string" }, true])
        );
        let descriptor = &definitions["Descriptor"]["properties"];
        assert_eq!(
            descriptor["params"],
            json!({ "type": ["object", "boolean"] })
        );
        assert_eq!(
            descriptor["result"],
            json!({ "type": ["object", "boolean"] })
        );
    }

    #[test]
    fn envelope_and_page_are_named_without_their_type_argument_with_data_unconstrained() {
        let definitions = definitions();
        assert!(!definitions.contains_key("Envelope_for_AnyValue"));
        assert!(!definitions.contains_key("Page_AnyValue"));
        let envelope = &definitions["Envelope"]["oneOf"];
        assert_eq!(envelope[0]["properties"]["ok"], json!({ "const": true }));
        assert_eq!(envelope[0]["properties"]["data"], json!(true));
        assert_eq!(
            envelope[0]["properties"]["meta"],
            json!({ "$ref": "#/$defs/Meta" })
        );
        assert_eq!(envelope[1]["properties"]["ok"], json!({ "const": false }));
        assert_eq!(
            definitions["Page"]["properties"]["data"]["items"],
            json!(true)
        );
        let text = serde_json::to_string(&definitions).unwrap();
        assert!(!text.contains("AnyValue"));
    }

    #[test]
    fn every_per_type_file_is_one_ref_into_the_document() {
        let files = files().unwrap();
        assert_eq!(files.len(), 32);
        for (name, content) in &files {
            if name == DOCUMENT {
                continue;
            }
            let root = name.strip_suffix(".json").unwrap();
            let value: Value = serde_json::from_str(content).unwrap();
            assert_eq!(
                value,
                json!({ "$ref": format!("keel.json#/$defs/{root}") }),
                "{name}"
            );
        }
        assert!(files.contains_key("Expr.json"));
        assert!(files.contains_key("Boot.json"));
    }

    #[test]
    fn check_refuses_a_document_with_a_property_removed_and_accepts_the_committed_directory() {
        run(&committed(), true).unwrap();

        let temporary = tempfile::tempdir().unwrap();
        for entry in fs::read_dir(committed()).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), temporary.path().join(entry.file_name())).unwrap();
        }
        let path = temporary.path().join(DOCUMENT);
        let mut document: Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let removed = document["$defs"]["Descriptor"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("module");
        assert!(removed.is_some());
        fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

        let error = run(temporary.path(), true).unwrap_err();
        match error {
            Error::Changed(files) => assert_eq!(files, [DOCUMENT]),
            other => panic!("expected the changed document, got {other}"),
        }
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            serde_json::to_string_pretty(&document).unwrap(),
            "check rewrites nothing"
        );
    }

    #[test]
    fn writing_creates_the_directory_and_removes_a_stale_per_type_file() {
        let temporary = tempfile::tempdir().unwrap();
        let out = temporary.path().join("schema");
        let outcome = run(&out, false).unwrap();
        assert_eq!(outcome.files.len(), 32);
        assert!(
            outcome
                .to_string()
                .starts_with("32 schema files written to")
        );
        let stale = out.join("Gone.json");
        fs::write(&stale, "{}\n").unwrap();
        let error = run(&out, true).unwrap_err();
        match error {
            Error::Changed(files) => assert_eq!(files, ["Gone.json"]),
            other => panic!("expected the stale file, got {other}"),
        }
        run(&out, false).unwrap();
        assert!(!stale.exists());
        let outcome = run(&out, true).unwrap();
        assert!(
            outcome
                .to_string()
                .ends_with(&format!("up to date in {}", out.display()))
        );
    }
}
