pub mod functions;

use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::{Db, Id, KeelError, Result, Secret};

use super::entity::login_key;
use super::password::{hex, random_bytes};
use super::session;

pub const KEY_PREFIX: &str = "kok_";
pub const MAX_TTL_SECONDS: u32 = 300;
pub const LOGIN_PATH: &str = "/login";
const KEY_LEN: usize = 32;

pub fn generate() -> Result<Secret> {
    Ok(Secret::new(format!(
        "{KEY_PREFIX}{}",
        hex(&random_bytes(KEY_LEN)?)
    )))
}

pub fn hash(key: &str) -> String {
    session::hash(key)
}

pub fn url_of(key: &str) -> String {
    format!("{LOGIN_PATH}?key={key}")
}

pub fn is_local_path(redirect: &str) -> bool {
    let mut chars = redirect.chars();
    chars.next() == Some('/')
        && chars.next() != Some('/')
        && !redirect.contains('\\')
        && !redirect.chars().any(char::is_control)
}

pub async fn issue(
    db: &Db,
    principal: Id,
    issued_by: Id,
    redirect: &str,
    ttl: u32,
    now: DateTime<Utc>,
) -> Result<Secret> {
    let key = generate()?;
    login_key::ActiveModel {
        key_hash: Set(hash(key.expose())),
        principal_id: Set(principal.to_string()),
        issued_by: Set(issued_by.to_string()),
        redirect: Set(redirect.to_owned()),
        expires_at: Set(now + Duration::seconds(i64::from(ttl))),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(db)
    .await?;
    Ok(key)
}

pub async fn redeem(
    db: &Db,
    key_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<login_key::Model>> {
    let mut rows = login_key::Entity::update_many()
        .col_expr(login_key::Column::UsedAt, Expr::value(now))
        .filter(login_key::Column::KeyHash.eq(key_hash))
        .filter(login_key::Column::UsedAt.is_null())
        .filter(login_key::Column::ExpiresAt.gt(now))
        .exec_with_returning(db)
        .await?;
    Ok(rows.pop())
}

pub fn principal_id(row: &login_key::Model) -> Result<Id> {
    row.principal_id.parse().map_err(|_| {
        KeelError::internal("auth.error.key_principal")
            .param("principal", row.principal_id.as_str())
    })
}

pub fn issued_by(row: &login_key::Model) -> Result<Id> {
    row.issued_by.parse().map_err(|_| {
        KeelError::internal("auth.error.key_issuer").param("issuer", row.issued_by.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_the_prefix_and_thirty_two_random_bytes_as_hex_placed_in_the_login_url() {
        let key = generate().expect("entropy");
        let text = key.expose();
        assert!(text.starts_with(KEY_PREFIX), "{text}");
        assert_eq!(text.len(), KEY_PREFIX.len() + KEY_LEN * 2);
        assert!(
            text[KEY_PREFIX.len()..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        );
        assert_eq!(hash(text).len(), 64);
        assert_ne!(hash(text), text);
        assert_eq!(url_of(text), format!("/login?key={text}"));
        assert_eq!(MAX_TTL_SECONDS, 300);
    }

    #[test]
    fn a_redirect_is_a_path_starting_with_one_slash_without_a_scheme_or_a_backslash() {
        for local in ["/", "/f/dns.zone.list", "/a?b=https://c", "/a#b", "/a b"] {
            assert!(is_local_path(local), "{local}");
        }
        for foreign in [
            "",
            "//evil.example",
            "https://evil.example",
            "evil.example/a",
            "/\\evil.example",
            "/a\\b",
            "/a\nb",
            "\\\\evil",
            "a",
        ] {
            assert!(!is_local_path(foreign), "{foreign}");
        }
    }
}
