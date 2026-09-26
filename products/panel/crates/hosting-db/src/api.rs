use keel::prelude::*;

#[async_trait]
pub trait Databases: Send + Sync {
    async fn of_user(&self, db: &Db, account: &Id, user: &Id) -> Result<Vec<String>>;
}
