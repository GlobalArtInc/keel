use std::collections::BTreeMap;

use keel_sdk::async_trait::async_trait;
use keel_sdk::limit::LimitDef;
use keel_sdk::runtime::{Contributions, Db};
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};
use keel_sdk::system::{AccountSource, LimitSource};
use keel_sdk::{KeelError, Result};
use keel_types::Id;
use serde_json::Value;

use super::StoredAccounts;

const VALUES: &str = "SELECT value FROM keel_limit WHERE key = $1 AND account_id = ANY($2)";

#[derive(Clone, Copy, Debug)]
struct Declared {
    default: Option<u64>,
    unlimited: bool,
}

#[derive(Debug)]
pub struct StoredLimits {
    declared: BTreeMap<String, Declared>,
}

impl StoredLimits {
    pub fn new(contributions: &Contributions) -> Self {
        Self {
            declared: contributions
                .of::<LimitDef>()
                .map(|def| {
                    (
                        def.key().to_owned(),
                        Declared {
                            default: def.default_value(),
                            unlimited: def.allows_unlimited(),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[async_trait]
impl LimitSource for StoredLimits {
    async fn effective(&self, key: &str, account: Id, db: &Db) -> Result<Option<u64>> {
        let declared = self
            .declared
            .get(key)
            .ok_or_else(|| KeelError::internal("keel.error.internal").param("limit", key))?;
        let chain: Vec<String> = StoredAccounts
            .ancestry(account, db)
            .await?
            .iter()
            .map(ToString::to_string)
            .collect();
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                VALUES,
                [key.into(), chain.into()],
            ))
            .await?;
        let mut stored = Vec::with_capacity(rows.len());
        for row in rows {
            stored.push(row.try_get::<Value>("", "value")?);
        }
        Ok(resolve(*declared, &stored))
    }
}

fn resolve(declared: Declared, stored: &[Value]) -> Option<u64> {
    let tightest = stored.iter().filter_map(Value::as_u64).min();
    let unlimited = declared.unlimited && stored.iter().any(Value::is_null);
    match tightest {
        Some(max) => Some(max),
        None if unlimited => None,
        None => declared.default,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const OPEN: Declared = Declared {
        default: Some(50),
        unlimited: true,
    };
    const CLOSED: Declared = Declared {
        default: Some(50),
        unlimited: false,
    };

    #[test]
    fn the_tightest_value_along_the_path_wins_and_the_default_stands_for_none() {
        assert_eq!(resolve(OPEN, &[]), Some(50));
        assert_eq!(resolve(OPEN, &[json!(100), json!(10)]), Some(10));
        assert_eq!(resolve(OPEN, &[json!(null), json!(7)]), Some(7));
        assert_eq!(resolve(OPEN, &[json!(null)]), None);
        assert_eq!(resolve(CLOSED, &[json!(null)]), Some(50));
        assert_eq!(resolve(CLOSED, &[json!("many")]), Some(50));
        let unlimited = Declared {
            default: None,
            unlimited: true,
        };
        assert_eq!(resolve(unlimited, &[]), None);
    }
}
