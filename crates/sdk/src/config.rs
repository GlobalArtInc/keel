use std::any::Any;
use std::fmt;
use std::sync::Arc;

use schemars::{JsonSchema, Schema, schema_for};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::services::ServiceKey;

pub type Section = Box<dyn Any + Send + Sync>;

type Parser = Arc<dyn Fn(Value) -> Result<Section, serde_json::Error> + Send + Sync>;
type Fallback = Arc<dyn Fn() -> Section + Send + Sync>;

#[derive(Clone)]
pub struct ConfigSection {
    key: String,
    schema: Schema,
    service: ServiceKey,
    parse: Parser,
    fallback: Fallback,
}

impl ConfigSection {
    pub fn new<T>(key: &str) -> Self
    where
        T: DeserializeOwned + JsonSchema + Default + Send + Sync + 'static,
    {
        let mut schema = schema_for!(T);
        schema.remove("$schema");
        Self {
            key: key.to_owned(),
            schema,
            service: ServiceKey::of::<T>(),
            parse: Arc::new(|value| {
                let parsed: T = serde_json::from_value(value)?;
                Ok(Box::new(Arc::new(parsed)) as Section)
            }),
            fallback: Arc::new(|| Box::new(Arc::new(T::default())) as Section),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    #[doc(hidden)]
    pub fn service(&self) -> ServiceKey {
        self.service
    }

    #[doc(hidden)]
    pub fn parse(&self, value: Value) -> Result<Section, serde_json::Error> {
        (self.parse)(value)
    }

    #[doc(hidden)]
    pub fn fallback(&self) -> Section {
        (self.fallback)()
    }
}

impl fmt::Debug for ConfigSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfigSection")
            .field("key", &self.key)
            .field("type", &self.service.name())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, JsonSchema, PartialEq)]
    #[serde(default, deny_unknown_fields)]
    struct Host {
        socket: String,
        retries: u8,
    }

    impl Default for Host {
        fn default() -> Self {
            Self {
                socket: "/run/keel/agent.sock".into(),
                retries: 3,
            }
        }
    }

    #[test]
    fn a_section_parses_its_type_falls_back_to_the_default_and_carries_the_schema() {
        let section = ConfigSection::new::<Host>("host");
        assert_eq!(section.key(), "host");
        assert_eq!(section.schema().get("type"), Some(&json!("object")));
        assert_eq!(section.schema().get("$schema"), None);
        assert_eq!(
            section.schema().get("additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(section.service(), ServiceKey::of::<Host>());
        let parsed = section
            .parse(json!({ "socket": "/tmp/agent.sock" }))
            .expect("the section parses");
        let host = parsed.downcast::<Arc<Host>>().expect("a Host");
        assert_eq!(
            *host,
            Arc::new(Host {
                socket: "/tmp/agent.sock".into(),
                retries: 3
            })
        );
        let fallback = section.fallback().downcast::<Arc<Host>>().expect("a Host");
        assert_eq!(*fallback, Arc::new(Host::default()));
        let error = section
            .parse(json!({ "sockte": "x" }))
            .expect_err("an unknown field is refused");
        assert!(error.to_string().contains("sockte"), "{error}");
        assert!(
            format!("{section:?}").starts_with("ConfigSection { key: \"host\", type: \""),
            "{section:?}"
        );
    }
}
