use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::Client;
use keel_sdk::{Db, Id, Principal, Result, Secret};
use sha2::{Digest, Sha256};

use super::entity::session;
use super::password::{hex, random_bytes};

pub const TOKEN_PREFIX: &str = "ks_";
const KIND_SESSION: &str = "session";
pub const REASON_LOGOUT: &str = "logout";
pub const REASON_REPLACED: &str = "replaced";
pub const IDLE_MINUTES: i64 = 30;
pub const ABSOLUTE_HOURS: i64 = 12;
const TOKEN_LEN: usize = 32;

pub fn idle() -> Duration {
    Duration::minutes(IDLE_MINUTES)
}

pub fn absolute() -> Duration {
    Duration::hours(ABSOLUTE_HOURS)
}

pub fn generate() -> Result<Secret> {
    Ok(Secret::new(format!(
        "{TOKEN_PREFIX}{}",
        hex(&random_bytes(TOKEN_LEN)?)
    )))
}

pub fn hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub fn is_token(value: &str) -> bool {
    value.starts_with(TOKEN_PREFIX)
}

pub async fn open(
    db: &Db,
    principal: &Principal,
    client: &Client,
    now: DateTime<Utc>,
) -> Result<Secret> {
    let token = generate()?;
    session::ActiveModel {
        id_hash: Set(hash(token.expose())),
        principal_id: Set(principal.id.to_string()),
        account_id: Set(principal.account_id.to_string()),
        actor_principal_id: Set(None),
        kind: Set(KIND_SESSION.to_owned()),
        ip: Set(client.ip.map(|ip| ip.to_string())),
        user_agent: Set(client.user_agent.clone()),
        created_at: Set(now),
        idle_until: Set(now + idle()),
        absolute_until: Set(now + absolute()),
        revoked_at: Set(None),
        reason: Set(None),
    }
    .insert(db)
    .await?;
    Ok(token)
}

pub async fn resolve(db: &Db, id_hash: &str, now: DateTime<Utc>) -> Result<Option<session::Model>> {
    let row = session::Entity::find_by_id(id_hash)
        .filter(session::Column::RevokedAt.is_null())
        .filter(session::Column::IdleUntil.gt(now))
        .filter(session::Column::AbsoluteUntil.gt(now))
        .one(db)
        .await?;
    Ok(row)
}

pub async fn touch(db: &Db, id_hash: &str, now: DateTime<Utc>) -> Result<()> {
    session::Entity::update_many()
        .col_expr(session::Column::IdleUntil, Expr::value(now + idle()))
        .filter(session::Column::IdHash.eq(id_hash))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn revoke(db: &Db, id_hash: &str, now: DateTime<Utc>, reason: &str) -> Result<bool> {
    let updated = session::Entity::update_many()
        .col_expr(session::Column::RevokedAt, Expr::value(now))
        .col_expr(session::Column::Reason, Expr::value(reason))
        .filter(session::Column::IdHash.eq(id_hash))
        .filter(session::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub fn principal_id(row: &session::Model) -> Result<Id> {
    row.principal_id.parse().map_err(|_| {
        keel_sdk::KeelError::internal("auth.error.session_principal")
            .param("principal", row.principal_id.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_the_prefix_and_thirty_two_random_bytes_as_hex_and_only_its_hash_is_kept() {
        let token = generate().expect("entropy");
        let text = token.expose();
        assert!(is_token(text));
        assert_eq!(text.len(), TOKEN_PREFIX.len() + TOKEN_LEN * 2);
        assert!(
            text[TOKEN_PREFIX.len()..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        let digest = hash(text);
        assert_eq!(digest.len(), 64);
        assert!(!digest.contains(TOKEN_PREFIX));
        assert_ne!(hash(generate().expect("entropy").expose()), digest);
        assert!(!is_token("kat_abc"));
    }
}
