use std::sync::Arc;
use std::time::Duration;

use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

use super::delivery::{Delivery, ERROR_EVENT, representable};
use super::entity::product;
use super::functions::ERROR_STORED;
use super::inbox;
use super::remote::Remote;

pub const PAUSE: Duration = Duration::from_secs(5);
pub const LIMIT: u32 = 100;
pub const ROUNDS: usize = 50;

async fn links(db: &Db) -> Result<Vec<product::Model>> {
    Ok(product::Entity::find()
        .filter(product::Column::PullSeq.is_not_null())
        .filter(product::Column::Url.is_not_null())
        .order_by_asc(product::Column::Id)
        .all(db)
        .await?)
}

async fn pull(delivery: &Delivery, row: &product::Model) -> Result<usize> {
    let seams = delivery.seams()?;
    let (Some(url), Some(start)) = (row.url.as_deref(), row.pull_seq) else {
        return Ok(0);
    };
    let stored = || KeelError::internal(ERROR_STORED).param("id", row.id.as_str());
    let token =
        String::from_utf8(seams.cipher.open(&row.outbound_token_enc)?).map_err(|_| stored())?;
    let remote = Remote::new(url)?;
    let mut after = u64::try_from(start).unwrap_or_default();
    let mut received = 0;
    for _ in 0..ROUNDS {
        let page = remote.feed(&token, after, LIMIT).await?;
        if page.next < after
            || page.data.iter().any(|event| {
                event.instance_id.to_string() != row.instance_id
                    || event.seq <= after
                    || event.seq > page.next
            })
        {
            return Err(KeelError::peer(ERROR_EVENT)
                .param("reason", "feed")
                .param("url", remote.base()));
        }
        let now = seams.clock.now();
        for event in &page.data {
            if !representable(event) {
                tracing::warn!(peer = %row.id, event = %event.id, at = %event.at, "an event of the feed carries a time outside the years 0 to 9999; it is skipped");
                continue;
            }
            if inbox::receive(&seams.db, &row.id, event, now).await? {
                received += 1;
            }
        }
        let advanced = product::Entity::update_many()
            .col_expr(
                product::Column::PullSeq,
                Expr::value(i64::try_from(page.next).unwrap_or(i64::MAX)),
            )
            .filter(product::Column::Id.eq(row.id.as_str()))
            .filter(product::Column::PullSeq.eq(i64::try_from(after).unwrap_or(i64::MAX)))
            .exec(&seams.db)
            .await?;
        if received > 0 {
            delivery.wake();
        }
        if advanced.rows_affected == 0 || page.next == after {
            break;
        }
        after = page.next;
    }
    Ok(received)
}

pub async fn run(delivery: Arc<Delivery>) {
    loop {
        tokio::time::sleep(PAUSE).await;
        match delivery.seams() {
            Ok(seams) => match links(&seams.db).await {
                Ok(rows) => {
                    for row in rows {
                        if let Err(error) = pull(&delivery, &row).await {
                            tracing::warn!(peer = %row.id, url = row.url, error = %error, "the feed of a peer could not be read");
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(error = %error, "the links to pull could not be read");
                }
            },
            Err(error) => tracing::error!(error = %error, "the peer feed consumer has not started"),
        }
    }
}
