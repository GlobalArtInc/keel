use std::sync::{Arc, OnceLock};

use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::system::{Authenticated, Authenticator, Client, Credential, PrincipalSource};
use keel_sdk::types::Via;
use keel_sdk::{Clock, Db, KeelError, Principal, Result, SettingsSource};

use super::entity::session::Model as SessionRow;
use super::policy::idle_of;
use super::{session, token};

#[derive(Default)]
pub struct CredentialAuthenticator {
    started: OnceLock<Started>,
}

struct Started {
    clock: Arc<dyn Clock>,
    principals: Arc<dyn PrincipalSource>,
    settings: Arc<dyn SettingsSource>,
}

impl CredentialAuthenticator {
    pub fn attach(
        &self,
        clock: Arc<dyn Clock>,
        principals: Arc<dyn PrincipalSource>,
        settings: Arc<dyn SettingsSource>,
    ) {
        let _ = self.started.set(Started {
            clock,
            principals,
            settings,
        });
    }

    async fn touch(&self, row: &SessionRow, db: &Db, now: DateTime<Utc>) -> Result<()> {
        let started = self.started()?;
        let account = row.account_id.parse().map_err(|_| {
            KeelError::internal("auth.error.session_account")
                .param("account", row.account_id.as_str())
        })?;
        let idle = idle_of(started.settings.as_ref(), account, db).await?;
        session::touch(db, &row.id_hash, now, idle).await
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal("auth.error.not_started"))
    }

    async fn session(&self, secret: &str, db: &Db) -> Result<Authenticated> {
        let started = self.started()?;
        let now = started.clock.now();
        let id_hash = session::hash(secret);
        let Some(row) = session::resolve(db, &id_hash, now).await? else {
            return Err(unauthenticated());
        };
        let principal_id = session::principal_id(&row)?;
        let Some(mut principal) = started.principals.by_id(principal_id, db).await? else {
            return Err(unauthenticated());
        };
        self.touch(&row, db, now).await?;
        principal.via = Via::Http;
        if let Some(delegated) = self.delegated(&row, principal.clone(), db, now).await? {
            principal = delegated;
        }
        Ok(Authenticated {
            principal,
            session: Some(id_hash),
        })
    }

    async fn delegated(
        &self,
        parent: &SessionRow,
        mut actor: Principal,
        db: &Db,
        now: DateTime<Utc>,
    ) -> Result<Option<Principal>> {
        let Some(su_id_hash) = parent.su_id_hash.as_deref() else {
            return Ok(None);
        };
        let Some(child) = session::resolve(db, su_id_hash, now).await? else {
            return Ok(None);
        };
        let started = self.started()?;
        let Some(mut principal) = started
            .principals
            .by_id(session::principal_id(&child)?, db)
            .await?
        else {
            return Ok(None);
        };
        self.touch(&child, db, now).await?;
        actor.via = Via::Su;
        principal.via = Via::Http;
        principal.actor = Some(Box::new(actor));
        Ok(Some(principal))
    }

    async fn token(&self, secret: &str, client: &Client, db: &Db) -> Result<Authenticated> {
        let started = self.started()?;
        let now = started.clock.now();
        let Some(row) = token::resolve(db, &token::hash(secret), now).await? else {
            return Err(unauthenticated());
        };
        if !token::allows(&row.ip_allow.0, client.ip)? {
            return Err(unauthenticated());
        }
        let principal_id = token::principal_id(&row)?;
        let ceiling = token::ceiling(&row)?;
        let Some(mut principal) = started.principals.by_id(principal_id, db).await? else {
            return Err(unauthenticated());
        };
        token::touch(db, &row.id, now).await?;
        principal.level = principal.level.min(ceiling);
        principal.scope = row.scope.0;
        principal.via = Via::Http;
        Ok(Authenticated {
            principal,
            session: None,
        })
    }
}

fn unauthenticated() -> KeelError {
    KeelError::unauthenticated("keel.error.unauthenticated")
}

#[async_trait]
impl Authenticator for CredentialAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        client: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        match credential {
            Credential::Bearer(secret) if token::is_token(secret.expose()) => {
                self.token(secret.expose(), client, db).await.map(Some)
            }
            Credential::Session(secret) if session::is_token(secret.expose()) => {
                self.session(secret.expose(), db).await.map(Some)
            }
            Credential::Bearer(_) | Credential::Session(_) => Ok(None),
        }
    }
}
