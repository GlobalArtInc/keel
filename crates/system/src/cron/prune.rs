use keel_sdk::prelude::*;

use super::api::CronJob;

pub const JOB_ID: &str = "cron.idempotency.prune";
pub const SCHEDULE: &str = "25 * * * *";
pub const FUNCTION: &str = "keel.idempotency.prune";

pub(super) fn job() -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, |run| async move {
        let answered = run.invoke(FUNCTION, json!({})).await?;
        tracing::debug!(pruned = %answered["pruned"], "the expired idempotency records are pruned");
        Ok(())
    })
}
