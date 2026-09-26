use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use crate::api::{Mailbox, Mailboxes};
use crate::host::maildir_of;
use crate::{domain, mailbox};

pub(crate) struct Lookup;

#[async_trait]
impl Mailboxes for Lookup {
    async fn of_domain(&self, db: &Db, account: &Id, domain: &str) -> Result<Vec<Mailbox>> {
        let Some(found) = domain::Entity::find()
            .filter(domain::Column::AccountId.eq(account.to_string()))
            .filter(domain::Column::Name.eq(domain))
            .one(db)
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(mailbox::Entity::find()
            .filter(mailbox::Column::DomainId.eq(found.id))
            .order_by_asc(mailbox::Column::Address)
            .all(db)
            .await?
            .into_iter()
            .map(|row| Mailbox {
                maildir: maildir_of(&row.id),
                address: row.address,
            })
            .collect())
    }
}
