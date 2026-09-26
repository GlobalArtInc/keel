use keel::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostingUser {
    pub id: Id,
    pub account: Id,
    pub name: String,
    pub home: Option<String>,
}

#[async_trait]
pub trait HostingUsers: Send + Sync {
    async fn get(&self, db: &Db, id: &Id) -> Result<Option<HostingUser>>;
}
