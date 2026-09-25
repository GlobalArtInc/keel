use std::time::Duration;

use keel_sdk::StatCollector;
use keel_sdk::limit::Unit;
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};

use super::accounts::SYSTEM_KIND;

pub(super) const ACCOUNTS: &str = "keel.accounts";
const PRINCIPALS: &str = "keel.principals";
const INTERVAL: Duration = Duration::from_secs(3_600);

pub(super) fn accounts() -> StatCollector {
    StatCollector::new(ACCOUNTS, Unit::Count, INTERVAL, |db| async move {
        count(
            &db,
            Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT count(*) FROM keel_account WHERE kind <> $1",
                [DbValue::from(SYSTEM_KIND)],
            ),
        )
        .await
    })
}

pub(super) fn principals() -> StatCollector {
    StatCollector::new(PRINCIPALS, Unit::Count, INTERVAL, |db| async move {
        count(
            &db,
            Statement::from_string(DbBackend::Postgres, "SELECT count(*) FROM keel_principal"),
        )
        .await
    })
}

async fn count(db: &Db, statement: Statement) -> Result<f64> {
    let rows = match db.query_one(statement).await? {
        Some(row) => row.try_get_by_index::<i64>(0)?,
        None => 0,
    };
    Ok(rows as f64)
}
