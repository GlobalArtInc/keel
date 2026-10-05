use std::sync::Arc;

use keel_sdk::async_trait::async_trait;
use keel_sdk::runtime::Db;
use keel_sdk::system::{Authenticated, Authenticator, Client, Credential};
use keel_sdk::{KeelError, ModuleDef, Registrar, Result};
use keel_types::{Level, Principal};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use crate::fixtures::{Who, owner_principal, stored_principal};

pub const PRINCIPAL_HEADER: &str = "x-keel-test-principal";

pub static MODULE: ModuleDef = ModuleDef {
    id: "testkit",
    version: crate::VERSION,
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

struct FixtureAuthenticator;

fn init(r: &mut Registrar) -> Result<()> {
    let authenticator: Arc<dyn Authenticator> = Arc::new(FixtureAuthenticator);
    r.contribute(authenticator);
    Ok(())
}

#[async_trait]
impl Authenticator for FixtureAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        let login = match credential {
            Credential::Bearer(secret) => secret.expose(),
            Credential::Session(_) => return Ok(None),
        };
        let principal = if login == Who::Owner.login() {
            let mut owner = owner_principal();
            owner.via = keel_types::Via::Http;
            Some(owner)
        } else {
            stored(db, login).await?
        };
        Ok(principal.map(|principal| Authenticated {
            principal,
            session: None,
            token: None,
        }))
    }
}

async fn stored(db: &Db, login: &str) -> Result<Option<Principal>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, account_id, level, role, lang FROM keel_principal WHERE login = $1",
            [login.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = row.try_get("", "id")?;
    let account: String = row.try_get("", "account_id")?;
    let level: String = row.try_get("", "level")?;
    let role: String = row.try_get("", "role")?;
    let lang: String = row.try_get("", "lang")?;
    let id = id
        .parse()
        .map_err(|_| KeelError::internal("testkit.error.principal_id").param("id", id.as_str()))?;
    let account = account.parse().map_err(|_| {
        KeelError::internal("testkit.error.account_id").param("account", account.as_str())
    })?;
    let level: Level = level
        .parse()
        .map_err(|_| KeelError::internal("testkit.error.level").param("level", level.as_str()))?;
    Ok(Some(stored_principal(
        id, login, level, account, &role, &lang,
    )))
}
