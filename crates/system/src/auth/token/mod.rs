pub mod functions;

use std::net::IpAddr;

use ipnet::IpNet;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, Set};
use keel_sdk::types::{Level, Scope};
use keel_sdk::{Db, Id, KeelError, Principal, Result, Secret};

use super::entity::token::{self, StoredNets, StoredScope};
use super::password::{hex, random_bytes};
use super::session;

pub const API_TOKEN_PREFIX: &str = "kat_";
const TOKEN_LEN: usize = 32;

pub struct Issue<'a> {
    pub principal: &'a Principal,
    pub name: &'a str,
    pub level: Level,
    pub scope: Scope,
    pub ip_allow: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub fn generate() -> Result<Secret> {
    Ok(Secret::new(format!(
        "{API_TOKEN_PREFIX}{}",
        hex(&random_bytes(TOKEN_LEN)?)
    )))
}

pub fn is_token(value: &str) -> bool {
    value.starts_with(API_TOKEN_PREFIX)
}

pub fn hash(token: &str) -> String {
    session::hash(token)
}

pub async fn issue(db: &Db, issue: Issue<'_>, now: DateTime<Utc>) -> Result<(Id, Secret)> {
    let id = Id::generate();
    let token = generate()?;
    token::ActiveModel {
        id: Set(id.to_string()),
        token_hash: Set(hash(token.expose())),
        principal_id: Set(issue.principal.id.to_string()),
        account_id: Set(issue.principal.account_id.to_string()),
        name: Set(issue.name.to_owned()),
        level: Set(issue.level.as_str().to_owned()),
        scope: Set(StoredScope(issue.scope)),
        ip_allow: Set(StoredNets(issue.ip_allow)),
        expires_at: Set(issue.expires_at),
        last_used_at: Set(None),
        created_at: Set(now),
        revoked_at: Set(None),
    }
    .insert(db)
    .await?;
    Ok((id, token))
}

pub async fn resolve(
    db: &Db,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<token::Model>> {
    let row = token::Entity::find()
        .filter(token::Column::TokenHash.eq(token_hash))
        .filter(token::Column::RevokedAt.is_null())
        .filter(
            Condition::any()
                .add(token::Column::ExpiresAt.is_null())
                .add(token::Column::ExpiresAt.gt(now)),
        )
        .one(db)
        .await?;
    Ok(row)
}

pub async fn touch(db: &Db, id: &str, now: DateTime<Utc>) -> Result<()> {
    token::Entity::update_many()
        .col_expr(token::Column::LastUsedAt, Expr::value(now))
        .filter(token::Column::Id.eq(id))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn revoke(db: &Db, id: &str, now: DateTime<Utc>) -> Result<bool> {
    let updated = token::Entity::update_many()
        .col_expr(token::Column::RevokedAt, Expr::value(now))
        .filter(token::Column::Id.eq(id))
        .filter(token::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub fn allows(nets: &[String], ip: Option<IpAddr>) -> Result<bool> {
    if nets.is_empty() {
        return Ok(true);
    }
    let Some(ip) = ip.map(|ip| ip.to_canonical()) else {
        return Ok(false);
    };
    for net in nets {
        let parsed: IpNet = net
            .parse()
            .map_err(|_| KeelError::internal("auth.error.token_net").param("net", net.as_str()))?;
        if parsed.contains(&ip) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn principal_id(row: &token::Model) -> Result<Id> {
    row.principal_id.parse().map_err(|_| {
        KeelError::internal("auth.error.token_principal")
            .param("id", row.id.as_str())
            .param("principal", row.principal_id.as_str())
    })
}

pub fn ceiling(row: &token::Model) -> Result<Level> {
    row.level.parse().map_err(|_| {
        KeelError::internal("auth.error.token_level_unknown")
            .param("id", row.id.as_str())
            .param("level", row.level.as_str())
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
        assert!(!session::is_token(text));
        assert_eq!(text.len(), API_TOKEN_PREFIX.len() + TOKEN_LEN * 2);
        assert!(
            text[API_TOKEN_PREFIX.len()..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        let digest = hash(text);
        assert_eq!(digest.len(), 64);
        assert!(!digest.contains(API_TOKEN_PREFIX));
        assert_ne!(hash(generate().expect("entropy").expose()), digest);
    }

    #[test]
    fn an_empty_allow_list_admits_every_address_and_a_list_admits_only_its_networks() {
        let local: IpAddr = "127.0.0.1".parse().expect("an address");
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().expect("an address");
        let remote: IpAddr = "10.1.2.3".parse().expect("an address");
        let nets = |list: &[&str]| list.iter().map(|net| (*net).to_owned()).collect::<Vec<_>>();
        assert!(allows(&[], Some(local)).expect("no nets"));
        assert!(allows(&[], None).expect("no nets"));
        assert!(allows(&nets(&["127.0.0.0/8"]), Some(local)).expect("valid nets"));
        assert!(allows(&nets(&["127.0.0.0/8"]), Some(mapped)).expect("valid nets"));
        assert!(!allows(&nets(&["10.0.0.0/8"]), Some(local)).expect("valid nets"));
        assert!(allows(&nets(&["10.0.0.0/8", "127.0.0.1/32"]), Some(local)).expect("valid nets"));
        assert!(allows(&nets(&["10.0.0.0/8"]), Some(remote)).expect("valid nets"));
        assert!(!allows(&nets(&["10.0.0.0/8"]), None).expect("valid nets"));
        let error = allows(&nets(&["not-a-net"]), Some(local)).unwrap_err();
        assert_eq!(error.key(), "auth.error.token_net");
    }
}
