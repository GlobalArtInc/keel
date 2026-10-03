use std::sync::{Arc, OnceLock};

use keel_sdk::async_trait::async_trait;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::{
    AccountSource, Authenticated, Authenticator, Client, Credential, OnBehalf, PeerReach,
    PrincipalSource,
};
use keel_sdk::types::{Scope, Via};
use keel_sdk::{Clock, Db, Id, KeelError, Level, Principal, Result};

use super::entity::product;
use super::functions::scopes_of;
use super::on_behalf::{foreign_origin, names_peer, not_reached, parse};
use crate::auth::api::{PEER_TOKEN_PREFIX, token_hash};

pub const ERROR_PRINCIPAL: &str = "peer.error.principal";

#[derive(Default)]
pub struct PeerAuthenticator {
    started: OnceLock<Started>,
}

struct Started {
    clock: Arc<dyn Clock>,
    principals: Arc<dyn PrincipalSource>,
    accounts: Arc<dyn AccountSource>,
}

impl PeerAuthenticator {
    pub fn attach(
        &self,
        clock: Arc<dyn Clock>,
        principals: Arc<dyn PrincipalSource>,
        accounts: Arc<dyn AccountSource>,
    ) {
        let _ = self.started.set(Started {
            clock,
            principals,
            accounts,
        });
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal("peer.error.not_started"))
    }

    async fn reached(&self, instance: &str, principal: &Principal, db: &Db) -> Result<bool> {
        if !principal
            .origin
            .as_deref()
            .is_some_and(|origin| names_peer(origin, instance))
        {
            return Ok(false);
        }
        let account = self
            .started()?
            .accounts
            .by_id(principal.account_id, db)
            .await?;
        Ok(account
            .and_then(|account| account.origin)
            .is_some_and(|origin| names_peer(&origin, instance)))
    }

    async fn peer(&self, secret: &str, db: &Db) -> Result<Authenticated> {
        let started = self.started()?;
        let Some(row) = product::Entity::find()
            .filter(product::Column::InboundTokenHash.eq(token_hash(secret)))
            .one(db)
            .await?
        else {
            return Err(unauthenticated());
        };
        let link: Id = row.id.parse().map_err(|_| stored_principal(&row))?;
        let principal_id: Id = row
            .principal_id
            .as_deref()
            .and_then(|id| id.parse().ok())
            .ok_or_else(|| stored_principal(&row))?;
        let scope = scopes_of(&row)?;
        let Some(mut principal) = started.principals.by_id(principal_id, db).await? else {
            return Err(unauthenticated());
        };
        product::Entity::update_many()
            .col_expr(
                product::Column::LastSeenAt,
                Expr::value(started.clock.now()),
            )
            .filter(product::Column::Id.eq(row.id.as_str()))
            .exec(db)
            .await?;
        principal.level = scope.level.min(Level::Admin);
        principal.scope = Scope::Peer(scope);
        principal.via = Via::Peer(link);
        Ok(Authenticated {
            principal,
            session: None,
        })
    }
}

async fn instance_of(link: Id, db: &Db) -> Result<Option<String>> {
    Ok(product::Entity::find_by_id(link.to_string())
        .one(db)
        .await?
        .map(|row| row.instance_id))
}

fn unauthenticated() -> KeelError {
    KeelError::unauthenticated("keel.error.unauthenticated")
}

pub(super) fn stored_principal(row: &product::Model) -> KeelError {
    KeelError::internal(ERROR_PRINCIPAL).param("id", row.id.as_str())
}

#[async_trait]
impl OnBehalf for PeerAuthenticator {
    async fn act_for(
        &self,
        caller: &Principal,
        header: &str,
        db: &Db,
    ) -> Result<Option<Principal>> {
        let (Via::Peer(link), Scope::Peer(scope)) = (&caller.via, &caller.scope) else {
            return Ok(None);
        };
        if !scope.on_behalf {
            return Ok(None);
        }
        let customer = parse(header)?;
        let Some(instance) = instance_of(*link, db).await? else {
            return Err(unauthenticated());
        };
        if !names_peer(customer.origin, &instance) {
            return Err(foreign_origin(&customer));
        }
        let found = self
            .started()?
            .principals
            .by_external(customer.origin, customer.external_id, db)
            .await?;
        let Some(mut principal) = found else {
            return Err(not_reached(&customer));
        };
        if !self.reached(&instance, &principal, db).await? {
            return Err(not_reached(&customer));
        }
        principal.via = Via::Peer(*link);
        principal.scope = Scope::Peer(scope.clone());
        principal.actor = Some(Box::new(caller.clone()));
        Ok(Some(principal))
    }
}

#[async_trait]
impl PeerReach for PeerAuthenticator {
    async fn reaches(&self, link: Id, principal: &Principal, db: &Db) -> Result<bool> {
        match instance_of(link, db).await? {
            Some(instance) => self.reached(&instance, principal, db).await,
            None => Ok(false),
        }
    }

    async fn customer(&self, link: Id, external_id: &str, db: &Db) -> Result<Option<Principal>> {
        let Some(instance) = instance_of(link, db).await? else {
            return Ok(None);
        };
        let carrying = self
            .started()?
            .principals
            .of_external_id(external_id, db)
            .await?;
        let mut reached = Vec::new();
        for principal in carrying {
            if self.reached(&instance, &principal, db).await? {
                reached.push(principal);
            }
        }
        Ok(match <[Principal; 1]>::try_from(reached) {
            Ok([principal]) => Some(principal),
            Err(_) => None,
        })
    }
}

#[async_trait]
impl Authenticator for PeerAuthenticator {
    async fn authenticate(
        &self,
        credential: &Credential,
        _: &Client,
        db: &Db,
    ) -> Result<Option<Authenticated>> {
        match credential {
            Credential::Bearer(secret) if secret.expose().starts_with(PEER_TOKEN_PREFIX) => {
                self.peer(secret.expose(), db).await.map(Some)
            }
            Credential::Bearer(_) | Credential::Session(_) => Ok(None),
        }
    }
}
