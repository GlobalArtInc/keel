use keel_sdk::runtime::{Db, Ownership};
use keel_sdk::{AccountScope, FunctionDef, KeelError};
use keel_types::{Access, Id, Level, Principal};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use serde_json::Value;

use crate::registry::Registry;

pub fn level(access: &Access, principal: &Principal) -> Result<(), KeelError> {
    if principal.level >= access.level {
        return Ok(());
    }
    if principal.level == Level::Guest {
        return Err(KeelError::unauthenticated("keel.error.unauthenticated"));
    }
    Err(KeelError::forbidden("keel.error.level").param("level", access.level.as_str()))
}

pub fn permission(
    registry: &Registry,
    access: &Access,
    principal: &Principal,
) -> Result<(), KeelError> {
    let Some(key) = &access.permission else {
        return Ok(());
    };
    if principal.level >= Level::Admin {
        return Ok(());
    }
    let held = registry
        .permission(key)
        .is_some_and(|def| def.defaults.contains(&principal.level));
    if held {
        Ok(())
    } else {
        Err(KeelError::forbidden("keel.error.permission").param("permission", key.as_str()))
    }
}

pub async fn owned(
    db: &Db,
    scope: &AccountScope,
    def: &FunctionDef,
    params: &Value,
) -> Result<(), KeelError> {
    let Some(key) = def.key() else {
        return Ok(());
    };
    let owned = match def.ownership() {
        Ownership::Owned(owned) => owned,
        Ownership::Unowned => return Ok(()),
        Ownership::Undeclared => {
            return Err(KeelError::internal("keel.error.ownership_undeclared")
                .param("function", def.name()));
        }
    };
    let mut ids = match params.get(key) {
        None | Some(Value::Null) => return Ok(()),
        Some(Value::String(id)) => vec![id.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(id) => Ok(id.clone()),
                _ => Err(not_found()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(not_found()),
    };
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        return Ok(());
    }
    let [primary_key] = owned.primary_key.as_slice() else {
        return Err(KeelError::internal("keel.error.owned_composite_key")
            .param("function", def.name())
            .param("table", owned.table.as_str()));
    };
    let placeholders: Vec<String> = (1..=ids.len()).map(|n| format!("${n}")).collect();
    let sql = format!(
        r#"SELECT "{pk}" AS "id", "{account}" AS "account_id" FROM "{table}" WHERE "{pk}" IN ({placeholders})"#,
        pk = primary_key,
        account = owned.account_column,
        table = owned.table,
        placeholders = placeholders.join(", "),
    );
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            ids.iter().map(|id| DbValue::from(id.as_str())),
        ))
        .await?;
    if rows.len() != ids.len() {
        return Err(not_found());
    }
    let mut checked = Vec::new();
    for row in rows {
        let account: String = row.try_get("", "account_id")?;
        if checked.contains(&account) {
            continue;
        }
        let id: Id = account.parse().map_err(|_| not_found())?;
        if !scope.includes(db, id).await? {
            return Err(not_found());
        }
        checked.push(account);
    }
    Ok(())
}

fn not_found() -> KeelError {
    KeelError::not_found("keel.error.not_found")
}

#[cfg(test)]
mod tests {
    use keel_types::{ErrorCode, Scope, Via};

    use super::*;

    fn principal(level: Level) -> Principal {
        Principal {
            id: Id::generate(),
            login: "p".into(),
            level,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
        }
    }

    fn access(level: Level) -> Access {
        Access {
            level,
            permission: None,
            feature: None,
        }
    }

    #[test]
    fn a_guest_below_the_level_is_unauthenticated_and_anyone_else_is_forbidden() {
        assert_eq!(
            level(&access(Level::User), &principal(Level::Guest))
                .unwrap_err()
                .code(),
            ErrorCode::Unauthenticated
        );
        let error = level(&access(Level::Admin), &principal(Level::Reseller)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Forbidden);
        assert_eq!(error.params()["level"], "admin");
        level(&access(Level::Guest), &principal(Level::Guest)).unwrap();
        level(&access(Level::User), &principal(Level::User)).unwrap();
        level(&access(Level::User), &principal(Level::Owner)).unwrap();
    }
}
