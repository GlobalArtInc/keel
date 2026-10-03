use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter};
use keel_sdk::{Db, Id, Result};

use super::entity::{login_challenge, login_key, mail_token, password, session, token};
use super::session::REASON_REVOKED;

pub async fn credentials(db: &Db, principal: Id, now: DateTime<Utc>) -> Result<()> {
    let id = principal.to_string();
    token::Entity::update_many()
        .col_expr(token::Column::RevokedAt, Expr::value(now))
        .filter(token::Column::PrincipalId.eq(id.as_str()))
        .filter(token::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    session::Entity::update_many()
        .col_expr(session::Column::RevokedAt, Expr::value(now))
        .col_expr(session::Column::Reason, Expr::value(REASON_REVOKED))
        .filter(session::Column::RevokedAt.is_null())
        .filter(
            Condition::any()
                .add(session::Column::PrincipalId.eq(id.as_str()))
                .add(session::Column::ActorPrincipalId.eq(id.as_str())),
        )
        .exec(db)
        .await?;
    login_key::Entity::update_many()
        .col_expr(login_key::Column::UsedAt, Expr::value(now))
        .filter(login_key::Column::UsedAt.is_null())
        .filter(
            Condition::any()
                .add(login_key::Column::PrincipalId.eq(id.as_str()))
                .add(login_key::Column::IssuedBy.eq(id.as_str())),
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
