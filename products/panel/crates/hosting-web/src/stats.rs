use std::time::Duration;

use keel::prelude::*;
use keel::sdk::StatCollector;
use keel::sdk::limit::Unit;
use keel::sdk::sea_orm::{ConnectionTrait, DbBackend, Statement};

const INTERVAL: Duration = Duration::from_secs(3_600);
const PER_ACCOUNT: &str =
    "SELECT account_id, count(*) FROM hosting_site GROUP BY account_id ORDER BY account_id";

pub(crate) fn sites(key: &str) -> StatCollector {
    StatCollector::per_account(key, Unit::Count, INTERVAL, |db| async move {
        let rows = db
            .query_all(Statement::from_string(DbBackend::Postgres, PER_ACCOUNT))
            .await?;
        let mut counts = Vec::with_capacity(rows.len());
        for row in rows {
            let account: String = row.try_get_by_index(0)?;
            let sites: i64 = row.try_get_by_index(1)?;
            if let Ok(account) = account.parse::<Id>() {
                counts.push((account, sites as f64));
            }
        }
        Ok(counts)
    })
}
