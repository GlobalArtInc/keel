use std::collections::BTreeMap;

use keel_sdk::runtime::{Db, Ownership, PermissionDef};
use keel_sdk::system::{Features, PermissionCatalogue};
use keel_sdk::{AccountScope, FunctionDef, KeelError};
use keel_types::{Access, Id, Level, Mode, Principal, Via};
use sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};
use serde_json::Value;

use crate::registry::Registry;

const LICENSE: &str = "keel.error.license";

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
    if holds(registry, key, principal) {
        Ok(())
    } else {
        Err(KeelError::forbidden("keel.error.permission").param("permission", key.as_str()))
    }
}

pub(crate) fn feature(registry: &Registry, access: &Access) -> Result<(), KeelError> {
    match &access.feature {
        Some(feature) if !enabled(registry, feature) => Err(unlicensed(feature)),
        _ => Ok(()),
    }
}

pub(crate) fn needs(registry: &Registry, features: &[String]) -> Result<(), KeelError> {
    match features.iter().find(|feature| !enabled(registry, feature)) {
        Some(feature) => Err(unlicensed(feature)),
        None => Ok(()),
    }
}

fn enabled(registry: &Registry, feature: &str) -> bool {
    registry
        .services()
        .get::<dyn Features>()
        .is_some_and(|features| features.allows(feature))
}

fn unlicensed(feature: &str) -> KeelError {
    KeelError::license(LICENSE).param("feature", feature)
}

pub fn internal_only(function: &str) -> KeelError {
    KeelError::forbidden("keel.error.internal_only").param("function", function)
}

pub fn internal(def: &FunctionDef, principal: &Principal) -> Result<(), KeelError> {
    if def.is_internal() && principal.via != Via::Internal {
        return Err(internal_only(def.name()));
    }
    Ok(())
}

pub fn scope(function: &str, mode: Mode, principal: &Principal) -> Result<(), KeelError> {
    if !principal.scope.admits(function) {
        return Err(KeelError::forbidden("keel.error.token_scope").param("function", function));
    }
    if mode == Mode::Write && !principal.scope.admits_write() {
        return Err(KeelError::forbidden("keel.error.token_read_only").param("function", function));
    }
    Ok(())
}

pub fn holds(registry: &Registry, key: &str, principal: &Principal) -> bool {
    held(registry.permissions(), key, principal)
}

fn held(permissions: &BTreeMap<String, PermissionDef>, key: &str, principal: &Principal) -> bool {
    if principal.level >= Level::Admin {
        return true;
    }
    match principal.grants.get(key) {
        Some(granted) => *granted,
        None => permissions
            .get(key)
            .is_some_and(|def| def.defaults.contains(&principal.level)),
    }
}

pub struct RegisteredPermissions {
    permissions: BTreeMap<String, PermissionDef>,
}

impl RegisteredPermissions {
    pub fn new(permissions: BTreeMap<String, PermissionDef>) -> Self {
        Self { permissions }
    }
}

impl PermissionCatalogue for RegisteredPermissions {
    fn registered(&self, key: &str) -> bool {
        self.permissions.contains_key(key)
    }

    fn holds(&self, principal: &Principal, key: &str) -> bool {
        held(&self.permissions, key, principal)
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
    use std::collections::BTreeMap;

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
            grants: BTreeMap::new(),
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
    fn the_scope_guard_refuses_a_function_outside_the_masks_and_a_write_under_read_only() {
        let mut scoped = principal(Level::User);
        scoped.scope = Scope::Functions(vec!["keel.account.list".into(), "svc.*".into()]);
        scope("keel.account.list", Mode::Read, &scoped).unwrap();
        scope("svc.order.create", Mode::Write, &scoped).unwrap();
        let outside = scope("keel.principal.list", Mode::Read, &scoped).unwrap_err();
        assert_eq!(outside.code(), ErrorCode::Forbidden);
        assert_eq!(outside.key(), "keel.error.token_scope");
        assert_eq!(outside.params()["function"], "keel.principal.list");
        let mut read_only = principal(Level::User);
        read_only.scope = Scope::ReadOnly;
        scope("keel.principal.list", Mode::Read, &read_only).unwrap();
        let write = scope("keel.principal.update", Mode::Write, &read_only).unwrap_err();
        assert_eq!(write.code(), ErrorCode::Forbidden);
        assert_eq!(write.key(), "keel.error.token_read_only");
        assert_eq!(write.params()["function"], "keel.principal.update");
        scope(
            "keel.principal.update",
            Mode::Write,
            &principal(Level::User),
        )
        .unwrap();
    }

    #[test]
    fn a_stored_grant_answers_over_the_default_and_admins_hold_every_key() {
        let permissions = BTreeMap::from([(
            "dns.zone.manage".to_owned(),
            PermissionDef {
                key: "dns.zone.manage".into(),
                defaults: vec![Level::Reseller],
            },
        )]);
        let catalogue = RegisteredPermissions::new(permissions.clone());
        let user = principal(Level::User);
        let reseller = principal(Level::Reseller);
        assert!(!held(&permissions, "dns.zone.manage", &user));
        assert!(held(&permissions, "dns.zone.manage", &reseller));
        assert!(!held(&permissions, "dns.zone.unknown", &reseller));
        let mut granted = principal(Level::User);
        granted.grants.insert("dns.zone.manage".into(), true);
        granted.grants.insert("dns.zone.unknown".into(), true);
        assert!(held(&permissions, "dns.zone.manage", &granted));
        assert!(held(&permissions, "dns.zone.unknown", &granted));
        let mut denied = principal(Level::Reseller);
        denied.grants.insert("dns.zone.manage".into(), false);
        assert!(!held(&permissions, "dns.zone.manage", &denied));
        assert!(!catalogue.holds(&denied, "dns.zone.manage"));
        assert!(catalogue.holds(&granted, "dns.zone.manage"));
        assert!(catalogue.registered("dns.zone.manage"));
        assert!(!catalogue.registered("dns.zone.unknown"));
        for level in [Level::Admin, Level::Owner] {
            let mut top = principal(level);
            top.grants.insert("dns.zone.manage".into(), false);
            assert!(held(&permissions, "dns.zone.manage", &top));
            assert!(held(&permissions, "dns.zone.unknown", &top));
            assert!(catalogue.holds(&top, "dns.zone.unknown"));
        }
    }

    #[test]
    fn an_internal_function_admits_a_principal_arriving_internally_alone() {
        let def = keel_sdk::function::<Value, Value>("m.e.raise")
            .kind(keel_types::Kind::Action)
            .internal()
            .handler(|call: keel_sdk::Call<Value>| async move { Ok(call.params().clone()) });
        let mut caller = principal(Level::Owner);
        for via in [Via::Http, Via::Socket, Via::Su, Via::Plugin("x".into())] {
            caller.via = via;
            let error = internal(&def, &caller).unwrap_err();
            assert_eq!(error.code(), ErrorCode::Forbidden);
            assert_eq!(error.key(), "keel.error.internal_only");
            assert_eq!(error.params()["function"], "m.e.raise");
        }
        caller.via = Via::Internal;
        internal(&def, &caller).unwrap();
        let open = keel_sdk::function::<Value, Value>("m.e.list")
            .kind(keel_types::Kind::Action)
            .handler(|call: keel_sdk::Call<Value>| async move { Ok(call.params().clone()) });
        internal(&open, &principal(Level::Guest)).unwrap();
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
