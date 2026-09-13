use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Db;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::PrincipalSource;
use keel_sdk::{KeelError, Result};
use keel_types::{Id, Level, Principal, Scope, Via};

use super::entity::principal;

#[derive(Clone, Copy, Debug, Default)]
pub struct StoredPrincipals;

#[async_trait]
impl PrincipalSource for StoredPrincipals {
    async fn by_id(&self, id: Id, db: &Db) -> Result<Option<Principal>> {
        principal::Entity::find_by_id(id.to_string())
            .one(db)
            .await?
            .map(from_row)
            .transpose()
    }

    async fn by_login(&self, login: &str, db: &Db) -> Result<Option<Principal>> {
        principal::Entity::find()
            .filter(principal::Column::Login.eq(login))
            .one(db)
            .await?
            .map(from_row)
            .transpose()
    }
}

fn from_row(row: principal::Model) -> Result<Principal> {
    let id: Id = row.id.parse().map_err(|_| {
        KeelError::internal("keel.principal.error.id_unknown").param("id", row.id.as_str())
    })?;
    let account_id: Id = row.account_id.parse().map_err(|_| {
        KeelError::internal("keel.principal.error.account_unknown")
            .param("id", row.id.as_str())
            .param("account", row.account_id.as_str())
    })?;
    let level: Level = row.level.parse().map_err(|_| {
        KeelError::internal("keel.principal.error.level_unknown")
            .param("id", row.id.as_str())
            .param("level", row.level.as_str())
    })?;
    Ok(Principal {
        id,
        login: row.login,
        level,
        account_id,
        role: row.role,
        actor: None,
        scope: Scope::Full,
        via: Via::Internal,
        external_id: row.external_id,
        origin: row.origin,
        lang: row.lang,
    })
}

#[cfg(test)]
mod tests {
    use keel_sdk::sea_orm::prelude::DateTimeUtc;
    use keel_types::ErrorCode;

    use super::*;

    fn row(level: &str) -> principal::Model {
        principal::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".into(),
            account_id: "01J8A0000000000000000000A1".into(),
            login: "alice".into(),
            level: level.into(),
            role: "owner".into(),
            state: "active".into(),
            lang: "ru".into(),
            external_id: Some("client-42".into()),
            origin: Some("bill:01J9A".into()),
            acl_gen: 3,
            created_at: DateTimeUtc::default(),
            updated_at: DateTimeUtc::default(),
        }
    }

    #[test]
    fn a_stored_row_becomes_an_internal_principal_without_actor_or_scope() {
        let principal = from_row(row("reseller")).expect("a valid row");
        assert_eq!(principal.login, "alice");
        assert_eq!(principal.level, Level::Reseller);
        assert_eq!(principal.via, Via::Internal);
        assert_eq!(principal.scope, Scope::Full);
        assert!(principal.actor.is_none());
        assert_eq!(principal.lang, "ru");
        assert_eq!(principal.external_id.as_deref(), Some("client-42"));
        assert_eq!(principal.origin.as_deref(), Some("bill:01J9A"));
        assert_eq!(
            principal.account_id.to_string(),
            "01J8A0000000000000000000A1"
        );
    }

    #[test]
    fn a_row_with_an_unknown_level_is_an_internal_error_naming_it() {
        let error = from_row(row("king")).expect_err("king is not a level");
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), "keel.principal.error.level_unknown");
        assert_eq!(error.params()["level"], "king");
    }
}
