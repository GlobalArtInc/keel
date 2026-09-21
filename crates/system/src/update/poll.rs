use keel_sdk::serde_json::{self, json};
use keel_sdk::types::ErrorCode;
use keel_sdk::{KeelError, Result};

use super::super::cron::api::{CronJob, JobRun};
use super::CHECK;
use super::status::Status;

pub const JOB_ID: &str = "update.poll";
pub const SCHEDULE: &str = "30 4 * * *";
pub const PROBLEM_KEY: &str = "update.available";
const PROBLEM_RAISE: &str = "notify.problem.raise";
const PROBLEM_CLEAR: &str = "notify.problem.clear";
const FUNCTION_NOT_FOUND: &str = "keel.error.function_not_found";

pub(super) fn job() -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, |run| async move {
        let status: Status = serde_json::from_value(run.invoke(CHECK, json!({})).await?)?;
        if status.up_to_date {
            settle(&run, PROBLEM_CLEAR, pair(&status)).await
        } else {
            settle(&run, PROBLEM_RAISE, raise(&status)).await
        }
    })
}

fn pair(status: &Status) -> serde_json::Value {
    json!({ "key": PROBLEM_KEY, "subject": status.channel.as_str() })
}

fn raise(status: &Status) -> serde_json::Value {
    json!({
        "key": PROBLEM_KEY,
        "subject": status.channel.as_str(),
        "severity": "warning",
        "params": {
            "running": status.running,
            "available": status.available,
            "channel": status.channel.as_str(),
            "allowed": status.allowed,
            "blockedBy": status.blocked_by,
        },
        "source": JOB_ID,
    })
}

async fn settle(run: &JobRun, func: &str, params: serde_json::Value) -> Result<()> {
    match run.invoke(func, params).await {
        Ok(_) => Ok(()),
        Err(error) if without_notify(&error) => {
            tracing::debug!(
                job = JOB_ID,
                function = func,
                "the notify module is not loaded; the update problem is not kept"
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn without_notify(error: &KeelError) -> bool {
    error.code() == ErrorCode::NotFound && error.key() == FUNCTION_NOT_FOUND
}

#[cfg(test)]
mod tests {
    use super::super::channel::Channel;
    use super::super::status::Blocker;
    use super::*;

    #[test]
    fn the_job_runs_daily_and_its_raise_carries_the_versions() {
        let job = job();
        assert_eq!(job.id(), JOB_ID);
        assert_eq!(job.module(), "update");
        assert_eq!(job.schedule(), SCHEDULE);
        let status = Status {
            running: "0.1.0".into(),
            running_contract: Some(1),
            channel: Channel::Beta,
            channel_url: "http://c/".into(),
            available: "0.2.0".into(),
            notes: "n".into(),
            min_upgrade_from: "0.1.0".into(),
            contract: 1,
            up_to_date: false,
            allowed: false,
            blocked_by: Some(Blocker::Contract),
            previous: None,
        };
        let raised = raise(&status);
        assert_eq!(raised["key"], json!(PROBLEM_KEY));
        assert_eq!(raised["subject"], json!("beta"));
        assert_eq!(raised["severity"], json!("warning"));
        assert_eq!(raised["params"]["available"], json!("0.2.0"));
        assert_eq!(raised["params"]["blockedBy"], json!("contract"));
        assert_eq!(raised["source"], json!(JOB_ID));
        assert_eq!(
            pair(&status),
            json!({ "key": PROBLEM_KEY, "subject": "beta" })
        );
        assert!(without_notify(
            &KeelError::not_found(FUNCTION_NOT_FOUND).param("function", PROBLEM_RAISE)
        ));
        assert!(!without_notify(&KeelError::not_found(
            "keel.error.not_found"
        )));
        assert!(!without_notify(&KeelError::unavailable(FUNCTION_NOT_FOUND)));
    }
}
