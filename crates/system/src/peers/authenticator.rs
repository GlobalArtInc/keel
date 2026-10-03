use std::sync::{Arc, OnceLock};

use keel_sdk::async_trait::async_trait;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use keel_sdk::system::{Authenticated, Authenticator, Client, Credential, PrincipalSource};
use keel_sdk::types::{Scope, Via};
use keel_sdk::{Clock, Db, Id, KeelError, Level, Result};

use super::entity::product;
use super::functions::scopes_of;
use crate::auth::api::{PEER_TOKEN_PREFIX, token_hash};

pub const ERROR_PRINCIPAL: &str = "peer.error.principal";

#[derive(Default)]
pub struct PeerAuthenticator {
    started: OnceLock<Started>,
}

struct Started {
    clock: Arc<dyn Clock>,
    principals: Arc<dyn PrincipalSource>,
}

impl PeerAuthenticator {
    pub fn attach(&self, clock: Arc<dyn Clock>, principals: Arc<dyn PrincipalSource>) {
        let _ = self.started.set(Started { clock, principals });
    }

    async fn peer(&self, secret: &str, db: &Db) -> Result<Authenticated> {
        let started = self
            .started
            .get()
            .ok_or_else(|| KeelError::internal("peer.error.not_started"))?;
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

fn unauthenticated() -> KeelError {
    KeelError::unauthenticated("keel.error.unauthenticated")
}

pub(super) fn stored_principal(row: &product::Model) -> KeelError {
    KeelError::internal(ERROR_PRINCIPAL).param("id", row.id.as_str())
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
