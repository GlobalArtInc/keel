use async_trait::async_trait;
use keel_types::{Id, Level, Principal};
use sea_orm::sea_query::{Alias, Expr, JoinType, Query, SelectStatement};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Select, Statement, Value};

use crate::Result;

pub trait Owned: EntityTrait {
    fn account_column() -> Self::Column;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountScope {
    Nothing,
    Own(Id),
    Subtree(Id),
    All,
}

impl AccountScope {
    pub fn of(principal: &Principal) -> Self {
        match principal.level {
            Level::Guest => AccountScope::Nothing,
            Level::Service | Level::User => AccountScope::Own(principal.account_id),
            Level::Reseller => AccountScope::Subtree(principal.account_id),
            Level::Admin | Level::Owner => AccountScope::All,
        }
    }

    pub fn apply<E: Owned>(&self, query: Select<E>) -> Select<E> {
        let column = E::account_column();
        match self {
            AccountScope::All => query,
            AccountScope::Nothing => query.filter(Expr::cust("FALSE")),
            AccountScope::Own(account) => query.filter(column.eq(account.to_string())),
            AccountScope::Subtree(root) => query.filter(column.in_subquery(subtree_ids(*root))),
        }
    }

    pub async fn includes<C: ConnectionTrait>(&self, db: &C, account: Id) -> Result<bool> {
        let statement = match self {
            AccountScope::Nothing => return Ok(false),
            AccountScope::Own(own) => return Ok(*own == account),
            AccountScope::All => Statement::from_sql_and_values(
                db.get_database_backend(),
                r#"SELECT 1 FROM "keel_account" WHERE "id" = $1"#,
                [Value::from(account.to_string())],
            ),
            AccountScope::Subtree(root) => Statement::from_sql_and_values(
                db.get_database_backend(),
                r#"SELECT 1 FROM "keel_account" "a" JOIN "keel_account" "own" ON "own"."id" = $1 WHERE "a"."id" = $2 AND "a"."path" LIKE "own"."path" || '%'"#,
                [
                    Value::from(root.to_string()),
                    Value::from(account.to_string()),
                ],
            ),
        };
        Ok(db.query_one(statement).await?.is_some())
    }
}

fn subtree_ids(root: Id) -> SelectStatement {
    let account = Alias::new("keel_account");
    let node = Alias::new("a");
    let own = Alias::new("own");
    Query::select()
        .column((node.clone(), Alias::new("id")))
        .from_as(account.clone(), node)
        .join_as(
            JoinType::InnerJoin,
            account,
            own.clone(),
            Expr::col((own, Alias::new("id"))).eq(root.to_string()),
        )
        .and_where(Expr::cust(r#""a"."path" LIKE "own"."path" || '%'"#))
        .take()
}

pub fn scope_by_account<E: Owned>(query: Select<E>, principal: &Principal) -> Select<E> {
    AccountScope::of(principal).apply(query)
}

#[async_trait]
pub trait ScopePolicy: Send + Sync {
    async fn visible_accounts(&self, principal: &Principal) -> Result<AccountScope>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DefaultScopePolicy;

#[async_trait]
impl ScopePolicy for DefaultScopePolicy {
    async fn visible_accounts(&self, principal: &Principal) -> Result<AccountScope> {
        Ok(AccountScope::of(principal))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use keel_types::{Scope, Via};
    use sea_orm::{DbBackend, QueryTrait};

    mod zone {
        use sea_orm::entity::prelude::*;

        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "dns_zone")]
        pub struct Model {
            #[sea_orm(primary_key, auto_increment = false)]
            pub id: String,
            pub account_id: String,
            pub name: String,
        }

        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    impl Owned for zone::Entity {
        fn account_column() -> Self::Column {
            zone::Column::AccountId
        }
    }

    const ACCOUNT: &str = "01J8A0000000000000000000A1";

    fn principal(level: Level) -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().unwrap(),
            login: "p".into(),
            level,
            account_id: ACCOUNT.parse().unwrap(),
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

    fn sql(level: Level) -> String {
        scope_by_account(zone::Entity::find(), &principal(level))
            .build(DbBackend::Postgres)
            .to_string()
    }

    #[test]
    fn a_user_and_a_service_see_their_own_account() {
        for level in [Level::User, Level::Service] {
            assert_eq!(
                sql(level),
                format!(
                    r#"SELECT "dns_zone"."id", "dns_zone"."account_id", "dns_zone"."name" FROM "dns_zone" WHERE "dns_zone"."account_id" = '{ACCOUNT}'"#
                )
            );
        }
    }

    #[test]
    fn a_reseller_sees_the_subtree_under_its_account_by_path() {
        assert_eq!(
            sql(Level::Reseller),
            format!(
                r#"SELECT "dns_zone"."id", "dns_zone"."account_id", "dns_zone"."name" FROM "dns_zone" WHERE "dns_zone"."account_id" IN (SELECT "a"."id" FROM "keel_account" AS "a" INNER JOIN "keel_account" AS "own" ON "own"."id" = '{ACCOUNT}' WHERE "a"."path" LIKE "own"."path" || '%')"#
            )
        );
    }

    #[test]
    fn an_admin_and_the_owner_see_everything() {
        for level in [Level::Admin, Level::Owner] {
            assert_eq!(
                sql(level),
                r#"SELECT "dns_zone"."id", "dns_zone"."account_id", "dns_zone"."name" FROM "dns_zone""#
            );
        }
    }

    #[test]
    fn a_guest_sees_nothing() {
        assert_eq!(
            sql(Level::Guest),
            r#"SELECT "dns_zone"."id", "dns_zone"."account_id", "dns_zone"."name" FROM "dns_zone" WHERE FALSE"#
        );
    }

    #[test]
    fn the_scope_of_a_principal_follows_the_level_table() {
        let account: Id = ACCOUNT.parse().unwrap();
        assert_eq!(
            AccountScope::of(&principal(Level::Guest)),
            AccountScope::Nothing
        );
        assert_eq!(
            AccountScope::of(&principal(Level::User)),
            AccountScope::Own(account)
        );
        assert_eq!(
            AccountScope::of(&principal(Level::Reseller)),
            AccountScope::Subtree(account)
        );
        assert_eq!(
            AccountScope::of(&principal(Level::Admin)),
            AccountScope::All
        );
    }

    #[tokio::test]
    async fn the_default_policy_answers_the_level_table() {
        let scope = DefaultScopePolicy
            .visible_accounts(&principal(Level::Reseller))
            .await
            .unwrap();
        assert_eq!(scope, AccountScope::Subtree(ACCOUNT.parse().unwrap()));
    }
}
