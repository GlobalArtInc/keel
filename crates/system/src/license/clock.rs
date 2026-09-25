use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};

use crate::cron::api::{CronJob, JobRunExt};

use super::grant::Grant;

pub const KV_CLOCK: &str = "license_clock";
pub const JOB_ID: &str = "license.clock";
pub(super) const SCHEDULE: &str = "*/10 * * * *";
const ERROR_CLOCK: &str = "license.error.clock";

pub(super) async fn advance(db: &Db, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO keel_kv (key, value) VALUES ($1, $2) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value \
         WHERE (keel_kv.value #>> '{}')::timestamptz < (EXCLUDED.value #>> '{}')::timestamptz",
        [
            DbValue::from(KV_CLOCK),
            DbValue::Json(Some(Box::new(json!(now)))),
        ],
    ))
    .await?;
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT value FROM keel_kv WHERE key = $1",
            [DbValue::from(KV_CLOCK)],
        ))
        .await?
        .ok_or_else(|| KeelError::internal(ERROR_CLOCK))?;
    let value: Value = row.try_get("", "value")?;
    keel_sdk::serde_json::from_value(value.clone())
        .map_err(|_| KeelError::internal(ERROR_CLOCK).param("value", value.to_string()))
}

pub(super) fn job(grant: Arc<Grant>) -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, move |run| {
        let grant = grant.clone();
        async move { grant.advance(&run.db()).await }
    })
}
