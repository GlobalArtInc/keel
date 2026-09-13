use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use keel_sdk::chrono::{DateTime, Duration, Utc};
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::system::Client;
use keel_sdk::{Db, Id, KeelError, Result, Secret};

use super::super::entity::login_challenge;
use super::super::password::{hex, random_bytes};
use super::super::session;

pub const LIFETIME_MINUTES: i64 = 5;
pub const MAX_ATTEMPTS: u32 = 5;
const CHALLENGE_LEN: usize = 32;

pub fn lifetime() -> Duration {
    Duration::minutes(LIFETIME_MINUTES)
}

pub fn hash(challenge: &str) -> String {
    session::hash(challenge)
}

pub async fn create(db: &Db, principal: Id, client: &Client, now: DateTime<Utc>) -> Result<Secret> {
    let challenge = hex(&random_bytes(CHALLENGE_LEN)?);
    login_challenge::ActiveModel {
        challenge_hash: Set(hash(&challenge)),
        principal_id: Set(principal.to_string()),
        ip: Set(client.ip.map(|ip| ip.to_string())),
        expires_at: Set(now + lifetime()),
        used_at: Set(None),
        created_at: Set(now),
    }
    .insert(db)
    .await?;
    Ok(Secret::new(challenge))
}

pub async fn pending(
    db: &Db,
    challenge_hash: &str,
    now: DateTime<Utc>,
) -> Result<Option<login_challenge::Model>> {
    let row = login_challenge::Entity::find_by_id(challenge_hash)
        .filter(login_challenge::Column::UsedAt.is_null())
        .filter(login_challenge::Column::ExpiresAt.gt(now))
        .one(db)
        .await?;
    Ok(row)
}

pub async fn consume(db: &Db, challenge_hash: &str, now: DateTime<Utc>) -> Result<bool> {
    let updated = login_challenge::Entity::update_many()
        .col_expr(login_challenge::Column::UsedAt, Expr::value(now))
        .filter(login_challenge::Column::ChallengeHash.eq(challenge_hash))
        .filter(login_challenge::Column::UsedAt.is_null())
        .filter(login_challenge::Column::ExpiresAt.gt(now))
        .exec(db)
        .await?;
    Ok(updated.rows_affected > 0)
}

pub fn principal_id(row: &login_challenge::Model) -> Result<Id> {
    row.principal_id.parse().map_err(|_| {
        KeelError::internal("auth.error.totp_principal")
            .param("principal", row.principal_id.as_str())
    })
}

#[derive(Default)]
pub struct Attempts {
    entries: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    attempts: u32,
    until: DateTime<Utc>,
}

impl Attempts {
    pub fn admit(&self, challenge_hash: &str, until: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        entries.retain(|_, entry| entry.until > now);
        let entry = entries
            .entry(challenge_hash.to_owned())
            .or_insert(Entry { attempts: 0, until });
        entry.attempts += 1;
        entry.attempts <= MAX_ATTEMPTS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_admits_five_attempts_and_its_entry_leaves_with_its_window() {
        let attempts = Attempts::default();
        let start: DateTime<Utc> = "2026-09-13T10:00:00Z".parse().expect("an instant");
        let until = start + lifetime();
        for _ in 0..MAX_ATTEMPTS {
            assert!(attempts.admit("a", until, start));
        }
        assert!(!attempts.admit("a", until, start));
        assert!(!attempts.admit("a", until, start + Duration::seconds(1)));
        assert!(attempts.admit("b", until + lifetime(), until));
        let remaining: Vec<String> = attempts
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect();
        assert_eq!(remaining, ["b"]);
        assert!(attempts.admit("a", until + lifetime(), until));
    }

    #[test]
    fn a_challenge_is_thirty_two_random_bytes_as_hex_kept_only_as_its_hash() {
        let challenge = hex(&random_bytes(CHALLENGE_LEN).expect("entropy"));
        assert_eq!(challenge.len(), CHALLENGE_LEN * 2);
        assert_eq!(hash(&challenge).len(), 64);
        assert_ne!(hash(&challenge), challenge);
        assert_eq!(lifetime(), Duration::minutes(5));
    }
}
