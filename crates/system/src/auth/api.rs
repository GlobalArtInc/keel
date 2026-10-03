use keel_sdk::async_trait::async_trait;
use keel_sdk::{Db, Id, Result, Secret};

pub use super::limits::{Lockout, RateLimit};
pub use super::mail::SETTING_SHELL_URL;
pub use super::retire::credentials as retire_credentials;
pub use super::session::hash as token_hash;
pub use super::token::{PEER_TOKEN_PREFIX, generate_peer as peer_token, is_peer_token};

pub const LOCAL_METHOD: &str = "local";

#[async_trait]
pub trait LoginMethod: Send + Sync {
    fn id(&self) -> &str;
    async fn verify(&self, login: &str, secret: &Secret, db: &Db) -> Result<Id>;
}
