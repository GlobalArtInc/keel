use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, OnceLock};

use jsonschema::Validator;
use jsonschema::paths::LocationSegment;
use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::serde_json::{Map, Value};
use keel_sdk::system::AccountSource;
use keel_sdk::types::{ErrorCode, FieldError};
use keel_sdk::{Db, Id, KeelError, Level, Result, SettingDef, SettingsSource};

use super::entity::value;

#[derive(Default)]
pub struct Store {
    started: OnceLock<Started>,
}

struct Started {
    definitions: BTreeMap<String, Definition>,
    accounts: Arc<dyn AccountSource>,
}

pub struct Definition {
    def: SettingDef,
    validator: Validator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub value: Option<Value>,
    pub scope: Option<Id>,
}

pub struct Chain {
    ancestry: Vec<Id>,
    rows: Vec<value::Model>,
}

impl fmt::Debug for Definition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Definition")
            .field("def", &self.def)
            .finish()
    }
}

impl Definition {
    pub fn def(&self) -> &SettingDef {
        &self.def
    }

    pub fn check(&self, candidate: &Value) -> Result<()> {
        let fields: Vec<FieldError> = self
            .validator
            .iter_errors(candidate)
            .map(|error| field_error(&error))
            .collect();
        if fields.is_empty() {
            Ok(())
        } else {
            Err(KeelError::new(ErrorCode::Validation).fields(fields))
        }
    }
}

fn field_error(error: &jsonschema::ValidationError<'_>) -> FieldError {
    let mut path = "value".to_owned();
    for segment in error.instance_path().segments() {
        match segment {
            LocationSegment::Property(name) => {
                path.push('.');
                path.push_str(&name);
            }
            LocationSegment::Index(index) => path.push_str(&format!("[{index}]")),
        }
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
    let mut params = Map::new();
    params.insert("value".to_owned(), error.instance().as_ref().clone());
    FieldError {
        path,
        code: Some(format!("schema.{keyword}")),
        key: Some(format!("keel.schema.{keyword}")),
        params,
    }
}

impl Store {
    pub fn attach<'a>(
        &self,
        definitions: impl IntoIterator<Item = &'a SettingDef>,
        accounts: Arc<dyn AccountSource>,
    ) -> Result<()> {
        let mut compiled = BTreeMap::new();
        for def in definitions {
            let schema = def.schema().as_value();
            let validator = jsonschema::validator_for(schema).map_err(|error| {
                KeelError::internal("settings.error.schema")
                    .param("key", def.key())
                    .source(error)
            })?;
            compiled.insert(
                def.key().to_owned(),
                Definition {
                    def: def.clone(),
                    validator,
                },
            );
        }
        let _ = self.started.set(Started {
            definitions: compiled,
            accounts,
        });
        Ok(())
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal("settings.error.not_started"))
    }

    pub fn definition(&self, key: &str) -> Result<Option<&Definition>> {
        Ok(self.started()?.definitions.get(key))
    }

    pub fn definitions(&self) -> Result<impl Iterator<Item = &Definition>> {
        Ok(self.started()?.definitions.values())
    }

    pub fn visible(&self, level: Level) -> Result<impl Iterator<Item = &Definition>> {
        Ok(self
            .definitions()?
            .filter(move |definition| definition.def.level_value() <= level))
    }

    pub async fn chain(&self, account: Id, db: &Db) -> Result<Chain> {
        let ancestry = self.started()?.accounts.ancestry(account, db).await?;
        let rows = if ancestry.is_empty() {
            Vec::new()
        } else {
            value::Entity::find()
                .filter(
                    value::Column::ScopeId
                        .is_in(ancestry.iter().map(ToString::to_string).collect::<Vec<_>>()),
                )
                .all(db)
                .await?
        };
        Ok(Chain { ancestry, rows })
    }

    pub async fn write(
        &self,
        db: &Db,
        scope: Id,
        key: &str,
        candidate: Value,
        by: Id,
        now: DateTime<Utc>,
    ) -> Result<()> {
        value::Entity::insert(value::ActiveModel {
            scope_id: Set(scope.to_string()),
            key: Set(key.to_owned()),
            value: Set(candidate),
            updated_at: Set(now),
            updated_by: Set(by.to_string()),
        })
        .on_conflict(
            OnConflict::columns([value::Column::ScopeId, value::Column::Key])
                .update_columns([
                    value::Column::Value,
                    value::Column::UpdatedAt,
                    value::Column::UpdatedBy,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
        Ok(())
    }
}

impl Chain {
    pub fn resolve(&self, definition: &Definition) -> Resolved {
        let key = definition.def.key();
        let nearest = self.ancestry.iter().find_map(|scope| {
            let scope_id = scope.to_string();
            self.rows
                .iter()
                .find(|row| row.key == key && row.scope_id == scope_id)
                .map(|row| (*scope, row.value.clone()))
        });
        match nearest {
            Some((scope, value)) => Resolved {
                value: Some(value),
                scope: Some(scope),
            },
            None => Resolved {
                value: definition.def.default_value().cloned(),
                scope: None,
            },
        }
    }
}

#[async_trait]
impl SettingsSource for Store {
    async fn resolve(&self, key: &str, account: Id, db: &Db) -> Result<Option<Value>> {
        let Some(definition) = self.definition(key)? else {
            return Ok(None);
        };
        Ok(self.chain(account, db).await?.resolve(definition).value)
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::schemars::schema_for;
    use keel_sdk::serde_json::json;

    use super::*;

    struct NoAccounts;

    #[async_trait]
    impl AccountSource for NoAccounts {
        async fn ancestry(&self, _: Id, _: &Db) -> Result<Vec<Id>> {
            Ok(Vec::new())
        }
    }

    fn store() -> Store {
        let store = Store::default();
        store
            .attach(
                [
                    &SettingDef::new("t.ttl", schema_for!(u32)).default(3600),
                    &SettingDef::new("t.name", schema_for!(String)).level(Level::Reseller),
                ],
                Arc::new(NoAccounts),
            )
            .expect("the schemas compile");
        store
    }

    fn row(scope: &str, key: &str, value: Value) -> value::Model {
        value::Model {
            scope_id: scope.to_owned(),
            key: key.to_owned(),
            value,
            updated_at: DateTime::<Utc>::default(),
            updated_by: "x".into(),
        }
    }

    #[test]
    fn a_value_is_checked_against_the_schema_of_its_definition_on_the_value_field() {
        let store = store();
        let ttl = store
            .definition("t.ttl")
            .unwrap()
            .expect("t.ttl is defined");
        ttl.check(&json!(60)).expect("an integer fits");
        let error = ttl
            .check(&json!("soon"))
            .expect_err("a string does not fit");
        assert_eq!(error.code(), ErrorCode::Validation);
        let wire = error.to_call_error();
        assert_eq!(wire.fields[0].path, "value");
        assert_eq!(wire.fields[0].code.as_deref(), Some("schema.type"));
        assert_eq!(wire.fields[0].key.as_deref(), Some("keel.schema.type"));
        assert_eq!(wire.fields[0].params["value"], json!("soon"));
        let negative = ttl.check(&json!(-1)).expect_err("a negative does not fit");
        assert_eq!(
            negative.to_call_error().fields[0].code.as_deref(),
            Some("schema.minimum")
        );
        assert!(store.definition("t.other").unwrap().is_none());
        let visible: Vec<&str> = store
            .visible(Level::Reseller)
            .unwrap()
            .map(|definition| definition.def().key())
            .collect();
        assert_eq!(visible, ["t.name"]);
        assert_eq!(store.visible(Level::Admin).unwrap().count(), 2);
        assert_eq!(
            Store::default().definition("t.ttl").unwrap_err().key(),
            "settings.error.not_started"
        );
    }

    #[test]
    fn a_chain_answers_the_nearest_stored_value_up_the_ancestry_then_the_default() {
        let store = store();
        let ttl = store
            .definition("t.ttl")
            .unwrap()
            .expect("t.ttl is defined");
        let name = store
            .definition("t.name")
            .unwrap()
            .expect("t.name is defined");
        let a1: Id = "01J8A0000000000000000000A1".parse().unwrap();
        let reseller: Id = "01J8A0000000000000000000R1".parse().unwrap();
        let chain = Chain {
            ancestry: vec![a1, reseller, Id::ROOT_ACCOUNT],
            rows: vec![
                row(&Id::ROOT_ACCOUNT.to_string(), "t.ttl", json!(60)),
                row(&reseller.to_string(), "t.ttl", json!(120)),
                row(&Id::ROOT_ACCOUNT.to_string(), "t.other", json!(1)),
            ],
        };
        assert_eq!(
            chain.resolve(ttl),
            Resolved {
                value: Some(json!(120)),
                scope: Some(reseller)
            }
        );
        assert_eq!(
            chain.resolve(name),
            Resolved {
                value: None,
                scope: None
            }
        );
        let bare = Chain {
            ancestry: vec![Id::ROOT_ACCOUNT],
            rows: Vec::new(),
        };
        assert_eq!(
            bare.resolve(ttl),
            Resolved {
                value: Some(json!(3600)),
                scope: None
            }
        );
    }
}
