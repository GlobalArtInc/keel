mod eval;
mod parse;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::level::Level;

pub use eval::EvalError;
pub use parse::ParseError;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Expr {
    Eq(String, Value),
    Ne(String, Value),
    Gt(String, Value),
    Lt(String, Value),
    In(String, Vec<Value>),
    Matches(String, String),
    Truthy(String),
    Empty(String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

impl Expr {
    pub fn parse(source: &str) -> Result<Expr, ParseError> {
        parse::parse(source)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Context {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<ContextPrincipal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<ContextScreen>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextPrincipal {
    pub level: Level,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextScreen {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

impl Context {
    pub fn resolve(&self, variable: &str) -> Value {
        match variable.strip_prefix('$') {
            Some(system) => self.resolve_system(system),
            None => walk(&self.fields, variable.split('.')),
        }
    }

    fn resolve_system(&self, path: &str) -> Value {
        let mut segments = path.split('.');
        match segments.next() {
            Some("mode") => match (segments.next(), &self.mode) {
                (None, Some(mode)) => Value::String(mode.clone()),
                _ => Value::Null,
            },
            Some("principal") => match (segments.next(), segments.next(), &self.principal) {
                (Some("level"), None, Some(principal)) => {
                    Value::String(principal.level.as_str().to_owned())
                }
                _ => Value::Null,
            },
            Some("screen") => match (segments.next(), &self.screen) {
                (Some("params"), Some(screen)) => walk(&screen.params, segments),
                _ => Value::Null,
            },
            _ => Value::Null,
        }
    }
}

fn walk<'a>(root: &Map<String, Value>, mut segments: impl Iterator<Item = &'a str>) -> Value {
    let Some(first) = segments.next() else {
        return Value::Object(root.clone());
    };
    let mut current = match root.get(first) {
        Some(value) => value,
        None => return Value::Null,
    };
    for segment in segments {
        current = match current.get(segment) {
            Some(value) => value,
            None => return Value::Null,
        };
    }
    current.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ast_serialises_operators_by_name() {
        let ast = Expr::And(vec![
            Expr::Eq("kind".into(), json!("slave")),
            Expr::Not(Box::new(Expr::Truthy("dnssec".into()))),
        ]);
        assert_eq!(
            serde_json::to_value(&ast).unwrap(),
            json!({ "and": [ { "eq": ["kind", "slave"] }, { "not": { "truthy": "dnssec" } } ] })
        );
    }

    #[test]
    fn unknown_operator_fails_to_deserialise() {
        assert!(serde_json::from_value::<Expr>(json!({ "gte": ["ttl", 300] })).is_err());
        assert!(serde_json::from_value::<Expr>(json!({ "eq": ["ttl"] })).is_err());
        assert!(serde_json::from_value::<Expr>(json!("ttl")).is_err());
    }

    #[test]
    fn context_resolves_fields_and_system_variables() {
        let ctx: Context = serde_json::from_value(json!({
            "fields": { "ttl": 300, "owner": { "login": "alice" } },
            "mode": "edit",
            "principal": { "level": "reseller" },
            "screen": { "params": { "zone_id": "z1" } }
        }))
        .unwrap();
        assert_eq!(ctx.resolve("ttl"), json!(300));
        assert_eq!(ctx.resolve("owner.login"), json!("alice"));
        assert_eq!(ctx.resolve("missing"), Value::Null);
        assert_eq!(ctx.resolve("$mode"), json!("edit"));
        assert_eq!(ctx.resolve("$mode.x"), Value::Null);
        assert_eq!(ctx.resolve("$principal.level"), json!("reseller"));
        assert_eq!(ctx.resolve("$principal.login"), Value::Null);
        assert_eq!(ctx.resolve("$screen.params.zone_id"), json!("z1"));
        assert_eq!(ctx.resolve("$screen.params"), json!({ "zone_id": "z1" }));
        assert_eq!(ctx.resolve("$unknown"), Value::Null);
        assert_eq!(Context::default().resolve("$principal.level"), Value::Null);
    }
}
