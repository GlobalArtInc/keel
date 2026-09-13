use keel_types::Level;
use schemars::Schema;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct SettingDef {
    key: String,
    schema: Schema,
    default: Option<Value>,
    level: Level,
}

impl SettingDef {
    pub fn new(key: &str, schema: Schema) -> Self {
        Self {
            key: key.to_owned(),
            schema,
            default: None,
            level: Level::Admin,
        }
    }

    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default = Some(value.into());
        self
    }

    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn default_value(&self) -> Option<&Value> {
        self.default.as_ref()
    }

    pub fn level_value(&self) -> Level {
        self.level
    }

    pub fn module(&self) -> &str {
        self.key
            .split_once('.')
            .map_or(&self.key, |(module, _)| module)
    }
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::json;

    use super::*;

    #[test]
    fn a_setting_definition_carries_its_key_schema_default_and_level() {
        let def = SettingDef::new("dns.default_ttl", schema_for!(u32))
            .default(3600)
            .level(Level::Reseller);
        assert_eq!(def.key(), "dns.default_ttl");
        assert_eq!(def.module(), "dns");
        assert_eq!(def.schema().get("type"), Some(&json!("integer")));
        assert_eq!(def.default_value(), Some(&json!(3600)));
        assert_eq!(def.level_value(), Level::Reseller);
        let bare = SettingDef::new("dns.zone_dir", schema_for!(String));
        assert_eq!(bare.default_value(), None);
        assert_eq!(bare.level_value(), Level::Admin);
        assert_eq!(SettingDef::new("flat", schema_for!(bool)).module(), "flat");
    }
}
