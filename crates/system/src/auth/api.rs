use keel_sdk::async_trait::async_trait;
use keel_sdk::{Db, Id, Result, Secret};

pub const LOCAL_METHOD: &str = "local";

#[async_trait]
pub trait LoginMethod: Send + Sync {
    fn id(&self) -> &str;
    async fn verify(&self, login: &str, secret: &Secret, db: &Db) -> Result<Id>;
}
