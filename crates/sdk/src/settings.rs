use async_trait::async_trait;
use keel_types::{Id, Level};
use schemars::Schema;
use serde_json::Value;

use crate::db::Db;
use crate::error::Result;

#[async_trait]
pub trait SettingsSource: Send + Sync {
    async fn resolve(&self, key: &str, account: Id, db: &Db) -> Result<Option<Value>>;
}

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
    use std::sync::Arc;

    use keel_types::ErrorCode;
    use schemars::schema_for;
    use sea_orm::DatabaseConnection;
    use serde_json::json;

    use super::*;
    use crate::call::CallBuilder;
    use crate::services::Services;

    struct Fixed;

    #[async_trait]
    impl SettingsSource for Fixed {
        async fn resolve(&self, key: &str, account: Id, _: &Db) -> Result<Option<Value>> {
            Ok(match key {
                "dns.default_ttl" => Some(json!(3600)),
                "dns.account" => Some(json!(account)),
                _ => None,
            })
        }
    }

    #[tokio::test]
    async fn a_call_reads_a_setting_through_the_source_for_its_own_account() {
        let mut services = Services::new();
        services.provide::<dyn SettingsSource>(Arc::new(Fixed));
        let principal = crate::tests::principal(Level::User);
        let account = principal.account_id;
        let handle = CallBuilder::new(principal, "req-1")
            .services(Arc::new(services))
            .read(DatabaseConnection::Disconnected);
        let call = handle.call(());
        assert_eq!(call.setting::<u32>("dns.default_ttl").await.unwrap(), 3600);
        assert_eq!(call.setting::<Id>("dns.account").await.unwrap(), account);
        assert_eq!(
            call.setting::<Option<String>>("dns.nothing").await.unwrap(),
            None
        );
        let unset = call.setting::<String>("dns.nothing").await.unwrap_err();
        assert_eq!(unset.code(), ErrorCode::Internal);
        assert_eq!(unset.key(), "keel.error.setting_value");
        assert_eq!(unset.params()["key"], "dns.nothing");
        let mistyped = call.setting::<String>("dns.default_ttl").await.unwrap_err();
        assert_eq!(mistyped.key(), "keel.error.setting_value");
        let bare = crate::tests::read_handle();
        let missing = bare
            .call(())
            .setting::<u32>("dns.default_ttl")
            .await
            .unwrap_err();
        assert_eq!(missing.key(), "keel.error.service_missing");
    }

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
