use std::sync::{Arc, OnceLock};

use keel_sdk::async_trait::async_trait;
use keel_sdk::system::{Authenticated, Authenticator, Client, Credential, PrincipalSource};
use keel_sdk::types::Via;
use keel_sdk::{Clock, Db, KeelError, Result};

use super::session;

const API_TOKEN_PREFIX: &str = "kat_";

#[derive(Default)]
pub struct SessionAuthenticator {
    started: OnceLock<Started>,
}

struct Started {
    clock: Arc<dyn Clock>,
    principals: Arc<dyn PrincipalSource>,
}

impl SessionAuthenticator {
    pub fn attach(&self, clock: Arc<dyn Clock>, principals: Arc<dyn PrincipalSource>) {
        let _ = self.started.set(Started { clock, principals });
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal("auth.error.not_started"))
    }
}

fn unauthenticated() -> KeelError {
    KeelError::unauthenticated("keel.error.unauthenticated")
}

#[async_trait]
impl Authenticator for SessionAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        let token = match credential {
            Credential::Bearer(token) if token.expose().starts_with(API_TOKEN_PREFIX) => {
                return Err(unauthenticated());
            }
            Credential::Bearer(_) => return Ok(None),
            Credential::Session(token) if session::is_token(token.expose()) => token,
            Credential::Session(_) => return Ok(None),
        };
        let started = self.started()?;
        let now = started.clock.now();
        let id_hash = session::hash(token.expose());
        let Some(row) = session::resolve(db, &id_hash, now).await? else {
            return Err(unauthenticated());
        };
        let principal_id = session::principal_id(&row)?;
        let Some(mut principal) = started.principals.by_id(principal_id, db).await? else {
            return Err(unauthenticated());
        };
        session::touch(db, &id_hash, now).await?;
        principal.via = Via::Http;
        Ok(Some(Authenticated {
            principal,
            session: Some(id_hash),
        }))
    }
}
