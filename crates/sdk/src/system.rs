use async_trait::async_trait;
use keel_types::Principal;

use crate::db::Db;
use crate::error::Result;
use crate::secret::Secret;

#[derive(Clone, Debug)]
pub enum Credential {
    Bearer(Secret),
    Session(Secret),
}

#[async_trait]
pub trait Authenticator: Send + Sync {
    async fn authenticate(&self, credential: &Credential, db: &Db) -> Result<Option<Principal>>;
}
