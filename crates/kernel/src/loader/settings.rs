use std::collections::BTreeMap;
use std::fmt;

use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Contributions;
use keel_sdk::{Db, Result, SettingDef, SettingsSource};
use keel_types::Id;
use serde_json::Value;

pub struct DefaultSettings {
    defaults: BTreeMap<String, Value>,
}

impl DefaultSettings {
    pub fn new(contributions: &Contributions) -> Self {
        Self {
            defaults: contributions
                .of::<SettingDef>()
                .filter_map(|def| {
                    def.default_value()
                        .map(|value| (def.key().to_owned(), value.clone()))
                })
                .collect(),
        }
    }
}

#[async_trait]
impl SettingsSource for DefaultSettings {
    async fn resolve(&self, key: &str, _: Id, _: &Db) -> Result<Option<Value>> {
        Ok(self.defaults.get(key).cloned())
    }
}

impl fmt::Debug for DefaultSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.defaults.keys()).finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::schemars::schema_for;
    use keel_types::Level;
    use sea_orm::DatabaseConnection;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn the_default_source_answers_the_contributed_default_of_any_account_and_nothing_else() {
        let mut contributions = Contributions::default();
        contributions.add(SettingDef::new("dns.default_ttl", schema_for!(u32)).default(3600));
        contributions
            .add(SettingDef::new("dns.zone_dir", schema_for!(String)).level(Level::Reseller));
        let source = DefaultSettings::new(&contributions);
        assert_eq!(format!("{source:?}"), "[\"dns.default_ttl\"]");
        let db = Db::from_connection(DatabaseConnection::Disconnected);
        let a1: Id = "01J8A0000000000000000000A1".parse().unwrap();
        assert_eq!(
            source.resolve("dns.default_ttl", a1, &db).await.unwrap(),
            Some(json!(3600))
        );
        assert_eq!(
            source
                .resolve("dns.default_ttl", Id::ROOT_ACCOUNT, &db)
                .await
                .unwrap(),
            Some(json!(3600))
        );
        assert_eq!(source.resolve("dns.zone_dir", a1, &db).await.unwrap(), None);
        assert_eq!(source.resolve("dns.other", a1, &db).await.unwrap(), None);
    }
}
