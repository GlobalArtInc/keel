use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, Statement, Value as DbValue};

use crate::cron::api::{CronJob, JobRunExt};

use super::collectors::{Collector, Stats};
use super::time::floor;

pub const JOB_ID: &str = "stats.collect";
pub(super) const SCHEDULE: &str = "* * * * *";
const MINUTE: i64 = 60;

pub(super) fn job(stats: Arc<Stats>) -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, move |run| {
        let stats = stats.clone();
        async move { collect(&stats, &run.db(), run.scheduled_at()).await }
    })
}

async fn collect(stats: &Stats, db: &Db, scheduled_at: DateTime<Utc>) -> Result<()> {
    let at = floor(scheduled_at, MINUTE)?;
    let minute = at.timestamp().div_euclid(MINUTE);
    let mut failure = None;
    for collector in stats
        .collectors()?
        .iter()
        .filter(|collector| collector.is_due(minute))
    {
        if let Err(error) = sample(collector, db, at).await {
            tracing::warn!(
                key = collector.def.key(),
                error = error.key(),
                "a statistic could not be sampled"
            );
            failure.get_or_insert(error);
        }
    }
    failure.map_or(Ok(()), Err)
}

async fn sample(collector: &Collector, db: &Db, at: DateTime<Utc>) -> Result<()> {
    let key = collector.def.key();
    let values: Vec<Value> = collector
        .def
        .sample(db.clone())
        .await?
        .into_iter()
        .filter(|(subject, value)| {
            let finite = value.is_finite();
            if !finite {
                tracing::warn!(
                    key,
                    subject,
                    "a statistic sampled a value that is not finite"
                );
            }
            finite
        })
        .map(|(subject, value)| json!({ "subject": subject, "value": value }))
        .collect();
    if values.is_empty() {
        return Ok(());
    }
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO stats_sample (key, subject, at, value) \
         SELECT $1, s.subject, $2, s.value \
         FROM jsonb_to_recordset($3) AS s(subject TEXT, value DOUBLE PRECISION) \
         ON CONFLICT (key, subject, at) DO UPDATE SET value = EXCLUDED.value",
        [
            DbValue::from(key),
            DbValue::from(at),
            DbValue::Json(Some(Box::new(Value::Array(values)))),
        ],
    ))
    .await?;
    Ok(())
}
