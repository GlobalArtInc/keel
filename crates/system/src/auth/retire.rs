use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement,
};
use keel_sdk::system::{CallExt, PeerReach};
use keel_sdk::types::Via;
use keel_sdk::{Call, Clock, Db, Id, KeelError, Result};

use super::entity::{login_challenge, login_key, mail_token, password, session, token};
use super::session::REASON_REVOKED;
use super::token::resolve as live_token;

const MINT: &str = "SELECT pg_advisory_xact_lock_shared(hashtextextended('auth_link:' || $1, 0))";
const RETIRE: &str = "SELECT pg_advisory_xact_lock(hashtextextended('auth_link:' || $1, 0))";

pub fn parsed(link: Option<&str>) -> Option<Id> {
    link.and_then(|link| link.parse().ok())
}

async fn lock(db: &Db, sql: &str, link: Id) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [link.to_string().into()],
    ))
    .await?;
    Ok(())
}

pub async fn lineage<P>(call: &Call<P>) -> Result<Option<Id>> {
    let principal = call.principal();
    let token = call.client().and_then(|client| client.token.as_deref());
    let tx = call.tx()?;
    let (link, live) = match principal.via {
        Via::Peer(peer) => {
            let link = principal
                .actor
                .as_deref()
                .map_or(principal.id, |actor| actor.id);
            lock(tx, MINT, link).await?;
            let live = match token {
                Some(token) => {
                    call.require::<dyn PeerReach>()?
                        .holds(peer, token, tx)
                        .await?
                }
                None => false,
            };
            (link, live)
        }
        _ => {
            let Some(token) = token else {
                return Ok(None);
            };
            let minted = token::Entity::find()
                .filter(token::Column::TokenHash.eq(token))
                .one(tx)
                .await?
                .and_then(|row| parsed(row.link_principal_id.as_deref()));
            let Some(link) = minted else {
                return Ok(None);
            };
            lock(tx, MINT, link).await?;
            let now = call.require::<dyn Clock>()?.now();
            (link, live_token(tx, token, now).await?.is_some())
        }
    };
    if live {
        Ok(Some(link))
    } else {
        Err(KeelError::unauthenticated("keel.error.unauthenticated"))
    }
}

pub async fn credentials(db: &Db, principal: Id, now: DateTime<Utc>) -> Result<()> {
    lock(db, RETIRE, principal).await?;
    let id = principal.to_string();
    token::Entity::update_many()
        .col_expr(token::Column::RevokedAt, Expr::value(now))
        .filter(
            Condition::any()
                .add(token::Column::PrincipalId.eq(id.as_str()))
                .add(token::Column::LinkPrincipalId.eq(id.as_str())),
        )
        .filter(token::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    login_key::Entity::update_many()
        .col_expr(login_key::Column::UsedAt, Expr::value(now))
        .filter(login_key::Column::UsedAt.is_null())
        .filter(
            Condition::any()
                .add(login_key::Column::PrincipalId.eq(id.as_str()))
                .add(login_key::Column::IssuedBy.eq(id.as_str()))
                .add(login_key::Column::LinkPrincipalId.eq(id.as_str())),
        )
        .exec(db)
        .await?;
    session::Entity::update_many()
        .col_expr(session::Column::RevokedAt, Expr::value(now))
        .col_expr(session::Column::Reason, Expr::value(REASON_REVOKED))
        .filter(session::Column::RevokedAt.is_null())
        .filter(
            Condition::any()
                .add(session::Column::PrincipalId.eq(id.as_str()))
                .add(session::Column::ActorPrincipalId.eq(id.as_str()))
                .add(session::Column::LinkPrincipalId.eq(id.as_str())),
        )
        .exec(db)
        .await?;
    login_challenge::Entity::update_many()
        .col_expr(login_challenge::Column::UsedAt, Expr::value(now))
        .filter(login_challenge::Column::PrincipalId.eq(id.as_str()))
        .filter(login_challenge::Column::UsedAt.is_null())
        .exec(db)
        .await?;
    mail_token::Entity::update_many()
        .col_expr(mail_token::Column::UsedAt, Expr::value(now))
        .filter(mail_token::Column::PrincipalId.eq(id.as_str()))
        .filter(mail_token::Column::UsedAt.is_null())
        .exec(db)
        .await?;
    password::Entity::delete_many()
        .filter(password::Column::PrincipalId.eq(id.as_str()))
        .exec(db)
        .await?;
    Ok(())
}
