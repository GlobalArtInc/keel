use std::collections::BTreeMap;

use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Db;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_sdk::system::{AccountSource, PrincipalSource};
use keel_sdk::{KeelError, Result};
use keel_types::{Id, Level, Principal, Scope, Via};

use super::entity::{account, principal};

#[derive(Clone, Copy, Debug, Default)]
pub struct StoredPrincipals;

#[derive(Clone, Copy, Debug, Default)]
pub struct StoredAccounts;

#[async_trait]
impl AccountSource for StoredAccounts {
    async fn ancestry(&self, account: Id, db: &Db) -> Result<Vec<Id>> {
        let Some(row) = account::Entity::find_by_id(account.to_string())
            .one(db)
            .await?
        else {
            return Ok(Vec::new());
        };
        ancestry_of(&row.path)
    }
}

fn ancestry_of(path: &str) -> Result<Vec<Id>> {
    let mut chain: Vec<Id> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            segment.parse().map_err(|_| {
                KeelError::internal("keel.account.error.path_unknown").param("segment", segment)
            })
        })
        .collect::<Result<_>>()?;
    chain.reverse();
    chain.push(Id::ROOT_ACCOUNT);
    Ok(chain)
}

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

    async fn at_or_above(&self, level: Level, db: &Db) -> Result<Vec<Principal>> {
        let levels: Vec<&str> = Level::ALL
            .iter()
            .filter(|candidate| **candidate >= level)
            .map(|candidate| candidate.as_str())
            .collect();
        principal::Entity::find()
            .filter(principal::Column::Level.is_in(levels))
            .order_by_asc(principal::Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(from_row)
            .collect()
    }

    async fn of_account(&self, account: Id, db: &Db) -> Result<Vec<Principal>> {
        principal::Entity::find()
            .filter(principal::Column::AccountId.eq(account.to_string()))
            .order_by_asc(principal::Column::Id)
            .all(db)
            .await?
            .into_iter()
            .map(from_row)
            .collect()
    }

    async fn touch_acl(&self, id: Id, db: &Db) -> Result<()> {
        let touched = principal::Entity::update_many()
            .col_expr(
                principal::Column::AclGen,
                Expr::col(principal::Column::AclGen).add(1),
            )
            .filter(principal::Column::Id.eq(id.to_string()))
            .exec(db)
            .await?;
        if touched.rows_affected == 0 {
            return Err(KeelError::not_found("keel.error.not_found"));
        }
        Ok(())
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
        grants: BTreeMap::new(),
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
    fn the_ancestry_of_a_path_runs_from_the_account_itself_up_to_the_root() {
        let a1: Id = "01J8A0000000000000000000A1".parse().unwrap();
        let a2: Id = "01J8A0000000000000000000A2".parse().unwrap();
        assert_eq!(ancestry_of("/").unwrap(), [Id::ROOT_ACCOUNT]);
        assert_eq!(
            ancestry_of("/01J8A0000000000000000000A1/").unwrap(),
            [a1, Id::ROOT_ACCOUNT]
        );
        assert_eq!(
            ancestry_of("/01J8A0000000000000000000A1/01J8A0000000000000000000A2/").unwrap(),
            [a2, a1, Id::ROOT_ACCOUNT]
        );
        let error = ancestry_of("/not-an-id/").expect_err("the segment is not a ULID");
        assert_eq!(error.key(), "keel.account.error.path_unknown");
        assert_eq!(error.params()["segment"], "not-an-id");
    }

    #[test]
    fn a_row_with_an_unknown_level_is_an_internal_error_naming_it() {
        let error = from_row(row("king")).expect_err("king is not a level");
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), "keel.principal.error.level_unknown");
        assert_eq!(error.params()["level"], "king");
    }
}
