use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;
use serde::de::{
    self, Deserialize, DeserializeOwned, DeserializeSeed, Deserializer, EnumAccess,
    IntoDeserializer, MapAccess, SeqAccess, VariantAccess, Visitor,
};
use serde_json::Value;

const ERROR_ARGS: &str = "host.error.args";
const ERROR_RESULT: &str = "host.error.io";

pub trait AgentScope {
    fn product(&self) -> &str;

    fn policy(&self) -> &Value;

    fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError>;

    fn caller_uid(&self) -> u32;
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{key}: {detail}")]
pub struct AgentOpError {
    key: String,
    detail: String,
}

impl AgentOpError {
    pub fn new(key: &str, detail: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            detail: detail.into(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

type Handler = dyn Fn(&dyn AgentScope, Value) -> Result<Value, AgentOpError> + Send + Sync;

#[derive(Clone)]
pub struct AgentOp {
    name: String,
    class: Option<String>,
    args: Schema,
    result: Schema,
    handler: Arc<Handler>,
}

impl AgentOp {
    pub fn new<A, R>(
        name: &str,
        handler: impl Fn(&dyn AgentScope, A) -> Result<R, AgentOpError> + Send + Sync + 'static,
    ) -> Self
    where
        A: DeserializeOwned + JsonSchema,
        R: Serialize + JsonSchema,
    {
        let erased = move |scope: &dyn AgentScope, args: Value| {
            let args = A::deserialize(Strict(&args))
                .map_err(|error| AgentOpError::new(ERROR_ARGS, error.to_string()))?;
            let result = handler(scope, args)?;
            serde_json::to_value(result)
                .map_err(|error| AgentOpError::new(ERROR_RESULT, error.to_string()))
        };
        Self {
            name: name.to_owned(),
            class: None,
            args: schema_for!(A),
            result: schema_for!(R),
            handler: Arc::new(erased),
        }
    }

    pub fn policy(mut self, class: &str) -> Self {
        self.class = Some(class.to_owned());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn class(&self) -> &str {
        match &self.class {
            Some(class) => class,
            None => self
                .name
                .split_once('.')
                .map_or(self.name.as_str(), |(module, _)| module),
        }
    }

    pub fn args_schema(&self) -> &Schema {
        &self.args
    }

    pub fn result_schema(&self) -> &Schema {
        &self.result
    }

    pub fn run(&self, scope: &dyn AgentScope, args: Value) -> Result<Value, AgentOpError> {
        (self.handler)(scope, args)
    }
}

impl fmt::Debug for AgentOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentOp")
            .field("name", &self.name)
            .field("class", &self.class())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
struct Strict<'a>(&'a Value);

struct Entries<'a> {
    iter: serde_json::map::Iter<'a>,
    pending: Option<&'a Value>,
}

struct Items<'a>(std::slice::Iter<'a, Value>);

struct Tagged<'a> {
    variant: &'a str,
    value: &'a Value,
}

macro_rules! delegate {
    ($($method:ident)*) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                self.0.$method(visitor)
            }
        )*
    };
}

impl<'de> Deserializer<'de> for Strict<'de> {
    type Error = serde_json::Error;

    delegate! {
        deserialize_any deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32
        deserialize_i64 deserialize_i128 deserialize_u8 deserialize_u16 deserialize_u32
        deserialize_u64 deserialize_u128 deserialize_f32 deserialize_f64 deserialize_char
        deserialize_str deserialize_string deserialize_bytes deserialize_byte_buf
        deserialize_unit deserialize_map deserialize_identifier deserialize_ignored_any
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.deserialize_unit_struct(name, visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.0 {
            Value::Null => visitor.visit_none(),
            inner => visitor.visit_some(Strict(inner)),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let Value::Array(items) = self.0 else {
            return self.0.deserialize_seq(visitor);
        };
        let mut access = Items(items.iter());
        let value = visitor.visit_seq(&mut access)?;
        if access.0.as_slice().is_empty() {
            Ok(value)
        } else {
            Err(de::Error::invalid_length(
                items.len(),
                &"fewer elements in array",
            ))
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        let Value::Object(map) = self.0 else {
            return self.0.deserialize_struct(name, fields, visitor);
        };
        if let Some(unknown) = map.keys().find(|key| !fields.contains(&key.as_str())) {
            return Err(de::Error::unknown_field(unknown, fields));
        }
        let mut access = Entries {
            iter: map.iter(),
            pending: None,
        };
        let value = visitor.visit_map(&mut access)?;
        if access.iter.len() == 0 {
            Ok(value)
        } else {
            Err(de::Error::invalid_length(
                map.len(),
                &"fewer elements in map",
            ))
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        if let Value::Object(map) = self.0 {
            let mut entries = map.iter();
            if let (Some((variant, value)), None) = (entries.next(), entries.next()) {
                return visitor.visit_enum(Tagged { variant, value });
            }
        }
        self.0.deserialize_enum(name, variants, visitor)
    }
}

impl<'de> MapAccess<'de> for Entries<'de> {
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.iter.next() else {
            return Ok(None);
        };
        self.pending = Some(value);
        let key =
            <&str as IntoDeserializer<'de, serde_json::Error>>::into_deserializer(key.as_str());
        seed.deserialize(key).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(value) => seed.deserialize(Strict(value)),
            None => Err(de::Error::custom("a value was asked for before its key")),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

impl<'de> SeqAccess<'de> for Items<'de> {
    type Error = serde_json::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        self.0
            .next()
            .map(|item| seed.deserialize(Strict(item)))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.0.len())
    }
}

impl<'de> EnumAccess<'de> for Tagged<'de> {
    type Error = serde_json::Error;
    type Variant = Strict<'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let name =
            <&str as IntoDeserializer<'de, serde_json::Error>>::into_deserializer(self.variant);
        Ok((seed.deserialize(name)?, Strict(self.value)))
    }
}

impl<'de> VariantAccess<'de> for Strict<'de> {
    type Error = serde_json::Error;

    fn unit_variant(self) -> Result<(), Self::Error> {
        <()>::deserialize(self.0)
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        seed.deserialize(self)
    }

    fn tuple_variant<V: Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.deserialize_struct("", fields, visitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigTemplate {
    id: String,
    text: String,
    check: Option<String>,
}

impl ConfigTemplate {
    pub fn new(id: &str, text: &str) -> Self {
        Self {
            id: id.to_owned(),
            text: text.to_owned(),
            check: None,
        }
    }

    pub fn validate(mut self, check: &str) -> Self {
        self.check = Some(check.to_owned());
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn check(&self) -> Option<&str> {
        self.check.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    struct Scope(Value);

    impl AgentScope for Scope {
        fn product(&self) -> &str {
            "demo"
        }

        fn policy(&self) -> &Value {
            &self.0
        }

        fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
            Ok(PathBuf::from(format!("/{root}/{name}")))
        }

        fn caller_uid(&self) -> u32 {
            990
        }
    }

    #[derive(Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    struct Args {
        name: String,
    }

    #[derive(Serialize, JsonSchema)]
    struct Answer {
        path: String,
        product: String,
        greeting: String,
        owner: u32,
    }

    fn answer(scope: &dyn AgentScope, args: Args) -> Result<Answer, AgentOpError> {
        let greeting = scope
            .policy()
            .get("greeting")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentOpError::new("host.error.policy", "no greeting"))?;
        Ok(Answer {
            path: scope.resolve("zones", &args.name)?.display().to_string(),
            product: scope.product().to_owned(),
            greeting: greeting.to_owned(),
            owner: scope.caller_uid(),
        })
    }

    #[test]
    fn an_agent_op_decodes_its_arguments_runs_its_handler_and_encodes_its_result() {
        let op = AgentOp::new::<Args, Answer>("vm.libvirt.create", answer).policy("vm");
        assert_eq!(op.name(), "vm.libvirt.create");
        assert_eq!(op.class(), "vm");
        let scope = Scope(json!({ "greeting": "hello" }));
        assert_eq!(
            op.run(&scope, json!({ "name": "a" })).unwrap(),
            json!({ "path": "/zones/a", "product": "demo", "greeting": "hello", "owner": 990 })
        );
        let refused = op
            .run(&scope, json!({ "name": "a", "path": "/etc" }))
            .unwrap_err();
        assert_eq!(refused.key(), "host.error.args");
        assert!(refused.detail().contains("path"), "{refused}");
        let failed = op
            .run(&Scope(json!({})), json!({ "name": "a" }))
            .unwrap_err();
        assert_eq!(
            failed,
            AgentOpError::new("host.error.policy", "no greeting")
        );
        assert!(format!("{op:?}").contains("vm.libvirt.create"));
    }

    #[derive(Deserialize, JsonSchema)]
    struct Pin {
        line: u32,
    }

    #[derive(Deserialize, JsonSchema)]
    enum Mark {
        Line { at: u32 },
        Note(Pin),
        Off,
    }

    #[derive(Deserialize, JsonSchema)]
    struct Loose {
        name: String,
        #[serde(alias = "former")]
        current: Option<Pin>,
        pins: Vec<Pin>,
        mark: Option<Mark>,
    }

    fn weight(mark: &Mark) -> u32 {
        match mark {
            Mark::Line { at } => *at,
            Mark::Note(pin) => pin.line,
            Mark::Off => 0,
        }
    }

    fn count(_: &dyn AgentScope, args: Loose) -> Result<String, AgentOpError> {
        let total = args.pins.iter().map(|pin| pin.line).sum::<u32>()
            + args.current.map_or(0, |pin| pin.line)
            + args.mark.as_ref().map_or(0, weight);
        Ok(format!("{}:{total}", args.name))
    }

    #[test]
    fn an_agent_op_refuses_an_unknown_field_at_any_depth_of_a_type_that_ignores_them() {
        let op = AgentOp::new::<Loose, String>("vm.libvirt.count", count);
        let scope = Scope(json!({}));
        for accepted in [
            json!({ "name": "a", "pins": [{ "line": 1 }], "mark": { "Line": { "at": 3 } } }),
            json!({ "name": "a", "former": { "line": 1 }, "pins": [], "mark": "Off" }),
            json!({ "name": "a", "pins": [], "mark": { "Note": { "line": 2 } } }),
        ] {
            assert!(op.run(&scope, accepted.clone()).is_ok(), "{accepted}");
        }
        for (refused, field) in [
            (json!({ "name": "a", "pins": [], "extra": 1 }), "extra"),
            (json!({ "name": "a", "pins": [{ "line": 1, "x": 2 }] }), "x"),
            (
                json!({ "name": "a", "current": { "line": 1, "y": 2 }, "pins": [] }),
                "y",
            ),
            (
                json!({ "name": "a", "pins": [], "mark": { "Line": { "at": 1, "z": 2 } } }),
                "z",
            ),
            (
                json!({ "name": "a", "pins": [], "mark": { "Note": { "line": 1, "w": 2 } } }),
                "w",
            ),
        ] {
            let error = op.run(&scope, refused.clone()).unwrap_err();
            assert_eq!(error.key(), "host.error.args", "{refused}");
            assert!(
                error.detail().contains(&format!("unknown field `{field}`")),
                "{refused}: {error}"
            );
        }
        let error = op
            .run(&scope, json!({ "name": 5, "pins": [] }))
            .unwrap_err();
        assert!(error.detail().contains("invalid type"), "{error}");
    }

    #[test]
    fn an_agent_op_without_a_policy_runs_under_the_class_of_its_module() {
        let op = AgentOp::new::<Args, Answer>("vm.libvirt.create", answer);
        assert_eq!(op.class(), "vm");
        let schema = serde_json::to_value(op.args_schema()).unwrap();
        assert_eq!(schema["required"], json!(["name"]));
        let schema = serde_json::to_value(op.result_schema()).unwrap();
        assert!(schema["properties"].get("greeting").is_some(), "{schema}");
    }

    #[test]
    fn a_config_template_carries_its_id_text_and_named_check() {
        let template = ConfigTemplate::new("bind/zone.j2", "{{ origin }}\n");
        assert_eq!(template.check(), None);
        let template = template.validate("named.check");
        assert_eq!(template.id(), "bind/zone.j2");
        assert_eq!(template.text(), "{{ origin }}\n");
        assert_eq!(template.check(), Some("named.check"));
    }
}
