use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::schemars::{Schema, json_schema};
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};

use crate::cron::api::{CronJob, JobRunExt};

use super::collectors::Stats;
use super::time::{before, floor};

pub const AGGREGATE_JOB: &str = "stats.aggregate";
pub const PRUNE_JOB: &str = "stats.prune";
pub const RAW_DAYS_KEY: &str = "stats.raw_days";
pub const HOUR_DAYS_KEY: &str = "stats.hour_days";
pub(super) const AGGREGATE_SCHEDULE: &str = "5 * * * *";
pub(super) const PRUNE_SCHEDULE: &str = "20 0 * * *";
pub(super) const RAW_DAYS: u32 = 7;
pub(super) const HOUR_DAYS: u32 = 90;
const MAX_DAYS: u32 = 3_650;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;
const ERROR_DAYS: &str = "stats.error.days";

const AGGREGATE_HOURS: &str = "\
    WITH last AS ( \
        SELECT key, subject, max(bucket) AS bucket FROM stats_agg \
        WHERE resolution = 'hour' GROUP BY key, subject \
    ) \
    INSERT INTO stats_agg (key, subject, bucket, resolution, min, max, sum, count) \
    SELECT s.key, s.subject, date_trunc('hour', s.at, 'UTC'), 'hour', \
           min(s.value), max(s.value), sum(s.value), count(*) \
    FROM stats_sample s \
    LEFT JOIN last l ON l.key = s.key AND l.subject = s.subject \
    WHERE s.at < $1 AND (l.bucket IS NULL OR s.at >= l.bucket) \
    GROUP BY s.key, s.subject, date_trunc('hour', s.at, 'UTC') \
    ON CONFLICT (key, subject, resolution, bucket) DO UPDATE SET \
        min = EXCLUDED.min, max = EXCLUDED.max, sum = EXCLUDED.sum, count = EXCLUDED.count";

const AGGREGATE_DAYS: &str = "\
    WITH last AS ( \
        SELECT key, subject, max(bucket) AS bucket FROM stats_agg \
        WHERE resolution = 'day' GROUP BY key, subject \
    ) \
    INSERT INTO stats_agg (key, subject, bucket, resolution, min, max, sum, count) \
    SELECT h.key, h.subject, date_trunc('day', h.bucket, 'UTC'), 'day', \
           min(h.min), max(h.max), sum(h.sum), sum(h.count)::BIGINT \
    FROM stats_agg h \
    LEFT JOIN last l ON l.key = h.key AND l.subject = h.subject \
    WHERE h.resolution = 'hour' AND h.bucket < $1 AND (l.bucket IS NULL OR h.bucket >= l.bucket) \
    GROUP BY h.key, h.subject, date_trunc('day', h.bucket, 'UTC') \
    ON CONFLICT (key, subject, resolution, bucket) DO UPDATE SET \
        min = EXCLUDED.min, max = EXCLUDED.max, sum = EXCLUDED.sum, count = EXCLUDED.count";

const PRUNE_SAMPLES: &str = "DELETE FROM stats_sample WHERE at < $1";

const PRUNE_HOURS: &str = "DELETE FROM stats_agg WHERE resolution = 'hour' AND bucket < $1";

pub(super) fn days_schema() -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": MAX_DAYS })
}

pub(super) fn aggregate_job() -> CronJob {
    CronJob::new(AGGREGATE_JOB, AGGREGATE_SCHEDULE, move |run| async move {
        aggregate(&run.db(), run.scheduled_at()).await
    })
}

pub(super) fn prune_job(stats: Arc<Stats>) -> CronJob {
    CronJob::new(PRUNE_JOB, PRUNE_SCHEDULE, move |run| {
        let stats = stats.clone();
        async move { prune(&stats, &run.db(), run.scheduled_at()).await }
    })
}

async fn aggregate(db: &Db, now: DateTime<Utc>) -> Result<()> {
    execute(db, AGGREGATE_HOURS, floor(now, HOUR)?).await?;
    execute(db, AGGREGATE_DAYS, floor(now, DAY)?).await
}

async fn prune(stats: &Stats, db: &Db, now: DateTime<Utc>) -> Result<()> {
    let raw_days = days(stats, db, RAW_DAYS_KEY, RAW_DAYS).await?;
    let hour_days = days(stats, db, HOUR_DAYS_KEY, HOUR_DAYS).await?;
    aggregate(db, now).await?;
    execute(db, PRUNE_SAMPLES, floor(before(now, raw_days)?, HOUR)?).await?;
    execute(db, PRUNE_HOURS, floor(before(now, hour_days)?, DAY)?).await
}

async fn days(stats: &Stats, db: &Db, key: &str, fallback: u32) -> Result<u32> {
    let resolved = stats.settings()?.resolve(key, Id::ROOT_ACCOUNT, db).await?;
    let days = match resolved {
        Some(value) => keel_sdk::serde_json::from_value::<u32>(value.clone()).map_err(|_| {
            KeelError::internal(ERROR_DAYS)
                .param("key", key)
                .param("value", value.to_string())
        })?,
        None => fallback,
    };
    Ok(days.clamp(1, MAX_DAYS))
}

async fn execute(db: &Db, sql: &str, at: DateTime<Utc>) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        [DbValue::from(at)],
    ))
    .await?;
    Ok(())
}
