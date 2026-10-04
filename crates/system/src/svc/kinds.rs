use std::collections::BTreeSet;

use keel_sdk::prelude::*;
use keel_sdk::schemars::Schema;
use keel_sdk::{InstanceInfoSource, ServiceKindDef};

use super::functions::{self, CLOSE, LOGIN, OPEN, RESUME, SET_PARAMS, STAT, SUSPEND};
use super::{ERROR_KIND, KINDS};

const IDENTITY: [&str; 2] = ["origin", "externalId"];

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Catalogue {
    pub data: Vec<Entry>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Entry {
    pub kind: String,
    pub schema_version: u32,
    pub title: String,
    pub account: Value,
    pub params: Value,
    pub state: Value,
    pub functions: Functions,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Functions {
    pub open: String,
    pub suspend: String,
    pub resume: String,
    pub close: String,
    pub set_params: String,
    pub stat: String,
    pub login: String,
}

pub(super) fn derive(mut kinds: Vec<ServiceKindDef>, r: &mut Registrar) -> Result<()> {
    kinds.sort_by_key(|def| def.kind());
    let mut seen = BTreeSet::new();
    for def in &kinds {
        check(def)?;
        if !seen.insert(def.kind()) {
            return Err(refused(def, "it is contributed twice"));
        }
    }
    for def in &kinds {
        for function in functions::all(def) {
            r.function(function);
        }
        r.contribute(InstanceInfoSource::new().service_kind(def.kind()));
    }
    r.function(catalogue(&kinds));
    Ok(())
}

fn check(def: &ServiceKindDef) -> Result<()> {
    if !is_kind(def.kind()) {
        return Err(refused(
            def,
            "its name is not lowercase segments of letters, digits and underscores joined by dots",
        ));
    }
    if def.schema_version() == 0 || i32::try_from(def.schema_version()).is_err() {
        return Err(refused(
            def,
            "its schema version is not a number from 1 to 2147483647",
        ));
    }
    if !identified(&def.schemas().account) {
        return Err(refused(
            def,
            "its account does not require origin and externalId",
        ));
    }
    Ok(())
}

fn refused(def: &ServiceKindDef, reason: &str) -> KeelError {
    KeelError::internal(ERROR_KIND)
        .param("kind", def.kind())
        .param("reason", reason)
}

pub fn is_kind(name: &str) -> bool {
    name.split('.').all(|segment| {
        let mut chars = segment.chars();
        chars.next().is_some_and(|first| first.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    })
}

fn identified(account: &Schema) -> bool {
    let required = account
        .get("required")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let properties = account.get("properties").and_then(Value::as_object);
    IDENTITY.iter().all(|member| {
        required.iter().any(|name| name == member)
            && properties.is_some_and(|properties| properties.contains_key(*member))
    })
}

fn embedded(schema: &Schema) -> Value {
    let mut value = schema.as_value().clone();
    if let Value::Object(object) = &mut value {
        object.remove("$schema");
    }
    value
}

fn entry(def: &ServiceKindDef) -> Entry {
    let name = |verb: &str| functions::name(def.kind(), verb);
    let schemas = def.schemas();
    Entry {
        kind: def.kind().to_owned(),
        schema_version: def.schema_version(),
        title: def.title().to_owned(),
        account: embedded(&schemas.account),
        params: embedded(&schemas.params),
        state: embedded(&schemas.state),
        functions: Functions {
            open: name(OPEN),
            suspend: name(SUSPEND),
            resume: name(RESUME),
            close: name(CLOSE),
            set_params: name(SET_PARAMS),
            stat: name(STAT),
            login: name(LOGIN),
        },
    }
}

fn catalogue(kinds: &[ServiceKindDef]) -> FunctionDef {
    let catalogue = Catalogue {
        data: kinds.iter().map(entry).collect(),
    };
    function::<Nothing, Catalogue>(KINDS)
        .kind(Kind::Get)
        .level(Level::Admin)
        .unowned()
        .handler(move |_: Call<Nothing>| {
            let catalogue = catalogue.clone();
            async move { Ok(catalogue) }
        })
}
