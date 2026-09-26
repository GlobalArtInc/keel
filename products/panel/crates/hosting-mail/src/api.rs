use keel::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mailbox {
    pub address: String,
    pub maildir: String,
}

#[async_trait]
pub trait Mailboxes: Send + Sync {
    async fn of_domain(&self, db: &Db, account: &Id, domain: &str) -> Result<Vec<Mailbox>>;
}
