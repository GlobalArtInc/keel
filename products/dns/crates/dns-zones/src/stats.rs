use std::time::Duration;

use keel::prelude::*;
use keel::sdk::StatCollector;
use keel::sdk::limit::Unit;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, EntityTrait, PaginatorTrait, Statement};

use crate::zone;

const INTERVAL: Duration = Duration::from_secs(3_600);
const PER_ACCOUNT: &str =
    "SELECT account_id, count(*) FROM dns_zone GROUP BY account_id ORDER BY account_id";

pub(crate) fn zones(key: &str) -> StatCollector {
    StatCollector::per_account(key, Unit::Count, INTERVAL, |db| async move {
        let rows = db
            .query_all(Statement::from_string(DbBackend::Postgres, PER_ACCOUNT))
            .await?;
        let mut counts = Vec::with_capacity(rows.len());
        for row in rows {
            let account: String = row.try_get_by_index(0)?;
            let zones: i64 = row.try_get_by_index(1)?;
            if let Ok(account) = account.parse::<Id>() {
                counts.push((account, zones as f64));
            }
        }
        Ok(counts)
    })
}

pub(crate) fn total(key: &str) -> StatCollector {
    StatCollector::new(key, Unit::Count, INTERVAL, |db| async move {
        let zones = zone::Entity::find().count(&db).await?;
        Ok(zones as f64)
    })
}
