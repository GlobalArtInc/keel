use std::sync::{Arc, OnceLock};

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use keel_sdk::async_trait::async_trait;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::sea_query::OnConflict;
use keel_sdk::sea_orm::{EntityTrait, Set};
use keel_sdk::system::{PasswordStore, PrincipalSource};
use keel_sdk::{Clock, Db, Id, KeelError, Result, Secret, SettingsSource};

use rand::TryRng;
use rand::rngs::SysRng;

use super::entity::password;
use super::policy::SETTING_PASSWORD_MIN_LENGTH;

pub const HASH_PREFIX: &str = "$argon2id$";
pub const MIN_LENGTH: u32 = 12;
const DECOY_LEN: usize = 32;

pub struct Passwords {
    decoy: String,
    principals: OnceLock<Arc<dyn PrincipalSource>>,
    platform: OnceLock<Platform>,
}

struct Platform {
    settings: Arc<dyn SettingsSource>,
    clock: Arc<dyn Clock>,
}

pub fn credentials() -> KeelError {
    KeelError::unauthenticated("auth.error.credentials")
}

fn hashing(error: impl std::error::Error + Send + Sync + 'static) -> KeelError {
    KeelError::internal("auth.error.hashing").source(error)
}

fn joining(error: tokio::task::JoinError) -> KeelError {
    KeelError::internal("auth.error.hashing").source(error)
}

pub fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    SysRng.try_fill_bytes(&mut bytes).map_err(|error| {
        KeelError::internal("auth.error.entropy").source(std::io::Error::other(error.to_string()))
    })?;
    Ok(bytes)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn check_policy(password: &Secret, login: &str, min_length: u32) -> Result<()> {
    let text = password.expose();
    if text.chars().count() < min_length as usize {
        return Err(KeelError::validation("auth.error.password_short")
            .field("password")
            .param("min", min_length));
    }
    if text.to_lowercase() == login.to_lowercase() {
        return Err(KeelError::validation("auth.error.password_login").field("password"));
    }
    Ok(())
}

fn hash_now(password: &[u8]) -> Result<String> {
    Argon2::default()
        .hash_password(password)
        .map(|hash| hash.to_string())
        .map_err(hashing)
}

fn matches(hash: &str, password: &[u8]) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(hashing)?;
    match Argon2::default().verify_password(password, &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::PasswordInvalid) => Ok(false),
        Err(error) => Err(hashing(error)),
    }
}

pub async fn store(db: &Db, principal: Id, hash: String, now: DateTime<Utc>) -> Result<()> {
    password::Entity::insert(password::ActiveModel {
        principal_id: Set(principal.to_string()),
        hash: Set(hash),
        updated_at: Set(now),
    })
    .on_conflict(
        OnConflict::column(password::Column::PrincipalId)
            .update_columns([password::Column::Hash, password::Column::UpdatedAt])
            .to_owned(),
    )
    .exec(db)
    .await?;
    Ok(())
}

impl Passwords {
    pub fn new() -> Result<Self> {
        let decoy = hash_now(hex(&random_bytes(DECOY_LEN)?).as_bytes())?;
        Ok(Self {
            decoy,
            principals: OnceLock::new(),
            platform: OnceLock::new(),
        })
    }

    pub fn attach(
        &self,
        principals: Arc<dyn PrincipalSource>,
        settings: Arc<dyn SettingsSource>,
        clock: Arc<dyn Clock>,
    ) {
        let _ = self.principals.set(principals);
        let _ = self.platform.set(Platform { settings, clock });
    }

    pub async fn hash(&self, password: &Secret) -> Result<String> {
        let password = password.expose().as_bytes().to_vec();
        tokio::task::spawn_blocking(move || hash_now(&password))
            .await
            .map_err(joining)?
    }

    pub async fn check(&self, hash: &str, password: &Secret) -> Result<bool> {
        let hash = hash.to_owned();
        let password = password.expose().as_bytes().to_vec();
        tokio::task::spawn_blocking(move || matches(&hash, &password))
            .await
            .map_err(joining)?
    }

    pub async fn verify(&self, login: &str, password: &Secret, db: &Db) -> Result<Id> {
        let principals = self
            .principals
            .get()
            .ok_or_else(|| KeelError::internal("auth.error.not_started"))?;
        let principal = principals.by_login(login, db).await?;
        let stored = match &principal {
            Some(principal) => password::Entity::find_by_id(principal.id.to_string())
                .one(db)
                .await?
                .map(|row| row.hash),
            None => None,
        };
        let (hash, id) = self.chosen(principal.map(|principal| principal.id), stored);
        match (self.check(&hash, password).await?, id) {
            (true, Some(id)) => Ok(id),
            _ => Err(credentials()),
        }
    }

    fn chosen(&self, principal: Option<Id>, stored: Option<String>) -> (String, Option<Id>) {
        match (stored, principal) {
            (Some(hash), Some(id)) => (hash, Some(id)),
            _ => (self.decoy.clone(), None),
        }
    }
}

#[async_trait]
impl PasswordStore for Passwords {
    async fn set(&self, principal: Id, login: &str, password: &Secret, db: &Db) -> Result<()> {
        let platform = self
            .platform
            .get()
            .ok_or_else(|| KeelError::internal("auth.error.not_started"))?;
        let min_length = platform
            .settings
            .resolve(SETTING_PASSWORD_MIN_LENGTH, Id::ROOT_ACCOUNT, db)
            .await?
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(MIN_LENGTH);
        check_policy(password, login, min_length)?;
        let hash = self.hash(password).await?;
        store(db, principal, hash, platform.clock.now()).await
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn the_decoy_is_an_argon2id_hash_that_verifies_no_password() {
        let passwords = Passwords::new().expect("entropy is available");
        assert!(passwords.decoy.starts_with(HASH_PREFIX));
        assert!(!matches(&passwords.decoy, b"").expect("a valid hash"));
        assert!(!matches(&passwords.decoy, b"hunter2").expect("a valid hash"));
        let other = Passwords::new().expect("entropy is available");
        assert_ne!(passwords.decoy, other.decoy);
    }

    #[test]
    fn an_unknown_login_and_a_login_without_a_password_row_are_checked_against_the_decoy() {
        let passwords = Passwords::new().expect("entropy is available");
        let id = Id::generate();
        let stored = "$argon2id$stored".to_owned();
        assert_eq!(
            passwords.chosen(Some(id), Some(stored.clone())),
            (stored, Some(id))
        );
        assert_eq!(
            passwords.chosen(Some(id), None),
            (passwords.decoy.clone(), None)
        );
        assert_eq!(
            passwords.chosen(None, None),
            (passwords.decoy.clone(), None)
        );
    }

    #[test]
    fn a_hash_verifies_its_own_password_and_refuses_another() {
        let hash = hash_now(b"hunter2").expect("hashing works");
        assert!(hash.starts_with(HASH_PREFIX));
        assert!(matches(&hash, b"hunter2").expect("a valid hash"));
        assert!(!matches(&hash, b"hunter3").expect("a valid hash"));
        assert!(matches("$2b$not-a-phc-string", b"x").is_err());
    }

    #[keel_testkit::tokio::test]
    async fn a_wrong_password_and_an_unknown_login_are_one_answer_after_the_same_verify() {
        use keel_sdk::types::ErrorCode;
        use keel_testkit::{Harness, Who};

        let harness = Harness::new().module(&crate::auth::MODULE).boot().await;
        let a1 = harness.principal(Who::A1).id;
        let set = harness
            .call(
                Who::Admin,
                "auth.password.set",
                json!({ "principal": a1, "password": "correct-horse" }),
            )
            .await;
        assert!(set.data().is_some(), "{:?}", set.error());
        let passwords = Passwords::new().expect("entropy is available");
        let db = Db::from_connection(harness.db().clone());
        let services = harness.registry().services();
        let source = services
            .get::<dyn PrincipalSource>()
            .expect("the kernel provides the principal source");
        let settings = services
            .get::<dyn SettingsSource>()
            .expect("the kernel provides the settings source");
        let clock = services
            .get::<dyn Clock>()
            .expect("the kernel provides the clock");
        assert_eq!(
            passwords
                .verify("a1", &Secret::new("correct-horse"), &db)
                .await
                .unwrap_err()
                .key(),
            "auth.error.not_started"
        );
        let a2 = harness.principal(Who::A2).id;
        assert_eq!(
            passwords
                .set(a2, "a2", &Secret::new("correct-horse"), &db)
                .await
                .unwrap_err()
                .key(),
            "auth.error.not_started"
        );
        passwords.attach(source, settings, clock);
        assert_eq!(
            passwords
                .set(a2, "a2", &Secret::new("short"), &db)
                .await
                .unwrap_err()
                .key(),
            "auth.error.password_short"
        );
        passwords
            .set(a2, "a2", &Secret::new("correct-horse-2"), &db)
            .await
            .expect("the store hashes and writes the password");
        assert_eq!(
            passwords
                .verify("a2", &Secret::new("correct-horse-2"), &db)
                .await
                .unwrap(),
            a2
        );
        assert_eq!(
            passwords
                .verify("a1", &Secret::new("correct-horse"), &db)
                .await
                .unwrap(),
            a1
        );
        let wrong = passwords
            .verify("a1", &Secret::new("correct-house"), &db)
            .await
            .unwrap_err();
        let unknown = passwords
            .verify("nobody", &Secret::new("correct-horse"), &db)
            .await
            .unwrap_err();
        let no_row = passwords
            .verify(Who::A2.login(), &Secret::new("correct-horse"), &db)
            .await
            .unwrap_err();
        for error in [&wrong, &unknown, &no_row] {
            assert_eq!(error.code(), ErrorCode::Unauthenticated);
            assert_eq!(error.key(), "auth.error.credentials");
            assert_eq!(error.to_call_error(), wrong.to_call_error());
        }
        harness.finish().await;
    }

    #[test]
    fn the_policy_refuses_a_short_password_and_one_equal_to_the_login() {
        let short = check_policy(&Secret::new("hunter2"), "alice", 12).unwrap_err();
        assert_eq!(short.key(), "auth.error.password_short");
        assert_eq!(short.field_path(), Some("password"));
        assert_eq!(short.params()["min"], json!(12));
        let login =
            check_policy(&Secret::new("Alice-Wonderland"), "alice-wonderland", 12).unwrap_err();
        assert_eq!(login.key(), "auth.error.password_login");
        assert_eq!(login.field_path(), Some("password"));
        assert!(check_policy(&Secret::new("correct-horse"), "alice", 12).is_ok());
        assert!(check_policy(&Secret::new("hunter2"), "alice", 7).is_ok());
        assert!(check_policy(&Secret::new("ünïcödé-pässwörd"), "alice", 16).is_ok());
    }

    #[test]
    fn bytes_are_written_as_lowercase_hex() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(random_bytes(4).expect("entropy").len(), 4);
    }
}
