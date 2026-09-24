use keel::prelude::*;
use keel::sdk::sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use keel_system::notify::api::{ProblemSource, Raise, Severity};

use crate::{FAILED, zone};

#[derive(Serialize)]
#[serde(crate = "keel::sdk::serde")]
struct Failed {
    name: String,
    error: String,
}

pub(crate) fn source() -> ProblemSource {
    ProblemSource::new(FAILED, |db| async move { failed(&db).await })
}

async fn failed(db: &Db) -> Result<Vec<Raise>> {
    let rows = zone::Entity::find()
        .filter(zone::Column::State.eq(zone::STATE_ERROR))
        .order_by_asc(zone::Column::Name)
        .all(db)
        .await?;
    let mut raises = Vec::with_capacity(rows.len());
    for row in rows {
        let account: Id = row.account_id.parse().map_err(|_| {
            KeelError::internal("dns.zone.error.account").param("account", row.account_id.as_str())
        })?;
        raises.push(
            Raise::new(FAILED, &row.id, Severity::Critical)
                .params(Failed {
                    name: row.name,
                    error: row.error.unwrap_or_default(),
                })?
                .account(account),
        );
    }
    Ok(raises)
}
