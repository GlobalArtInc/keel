use chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::system::TaskRunner;

use crate::backup;
use crate::restart::{EXIT_STATUS, Handle, Requested};

pub const RESTART: &str = "keel.restart";
pub(super) const CLASS: &str = "restart";
const LABEL: &str = "restart";
const ERROR_INLINE: &str = "keel.error.restart_inline";
const ERROR_PENDING: &str = "keel.error.restart_pending";
const PROGRESS_DRAIN: &str = "keel.restart.progress.drain";
const PROGRESS_DUMP: &str = "keel.restart.progress.dump";
const PROGRESS_REQUEST: &str = "keel.restart.progress.request";
const STEPS: u64 = 3;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct RestartRequest {
    #[schemars(length(min = 1, max = 200))]
    pub(super) reason: String,
    #[serde(default)]
    pub(super) requested_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct Restarted {
    pub(super) reason: String,
    pub(super) requested_by: String,
    pub(super) applied: bool,
    pub(super) dump: Option<String>,
    pub(super) tables: u32,
    pub(super) left_running: u32,
    pub(super) exit_status: u8,
}

pub(super) fn restart() -> FunctionDef {
    function::<RestartRequest, Restarted>(RESTART)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .internal()
        .class(CLASS)
        .handler(|call: Call<RestartRequest>| async move { run(&call).await })
}

async fn run(call: &Call<RestartRequest>) -> Result<Restarted> {
    let task = call
        .task()
        .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
    let handle = call.require::<Handle>()?;
    let reason = call.params().reason.clone();
    let requested_by = call.principal().login.clone();
    if let Some(requested_at) = call.params().requested_at
        && requested_at < handle.booted_at()
    {
        task.log(&format!(
            "the restart requested at {} for {reason} happened already: this process started at {}",
            requested_at.to_rfc3339(),
            handle.booted_at().to_rfc3339()
        ))
        .await?;
        return Ok(Restarted {
            reason,
            requested_by,
            applied: false,
            dump: None,
            tables: 0,
            left_running: 0,
            exit_status: EXIT_STATUS,
        });
    }
    if let Some(pending) = handle.pending() {
        return Err(KeelError::conflict(ERROR_PENDING)
            .param("reason", pending.reason)
            .param("requestedBy", pending.requested_by));
    }
    let runner = call.require::<dyn TaskRunner>()?;
    task.progress(0, Some(STEPS), Some(PROGRESS_DRAIN)).await?;
    task.log(&format!("restart requested by {requested_by}: {reason}"))
        .await?;
    let still_running = runner.quiesce(task.id()).await?;
    if still_running.is_empty() {
        task.log("the task queue is drained; no other job runs")
            .await?;
    } else {
        task.log(&format!(
            "the task queue is drained; {} job(s) did not stop within the grace and will be interrupted by the restart",
            still_running.len()
        ))
        .await?;
    }
    let outcome =
        dump_and_request(call, &handle, &reason, &requested_by, still_running.len()).await;
    if outcome.is_err()
        && let Err(error) = runner.resume()
    {
        tracing::error!(error = %error, "the task queue could not resume after a refused restart");
    }
    outcome
}

async fn dump_and_request(
    call: &Call<RestartRequest>,
    handle: &Handle,
    reason: &str,
    requested_by: &str,
    left_running: usize,
) -> Result<Restarted> {
    let task = call
        .task()
        .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
    task.progress(1, Some(STEPS), Some(PROGRESS_DUMP)).await?;
    let backups = call.require::<backup::Handle>()?;
    let dumped = backups.pre_change(LABEL, reason).await?;
    task.log(&format!(
        "dumped {} table(s) of {} into {}",
        dumped.tables.len(),
        dumped.schema,
        dumped.dir.display()
    ))
    .await?;
    task.progress(2, Some(STEPS), Some(PROGRESS_REQUEST))
        .await?;
    let now = call.require::<dyn Clock>()?.now();
    handle
        .request(Requested {
            reason: reason.to_owned(),
            requested_by: requested_by.to_owned(),
            task: Some(task.id()),
            at: now,
        })
        .map_err(|pending| {
            KeelError::conflict(ERROR_PENDING)
                .param("reason", pending.reason)
                .param("requestedBy", pending.requested_by)
        })?;
    task.log(&format!(
        "the service exits with status {EXIT_STATUS} when this task ends and the service manager starts it again"
    ))
    .await?;
    task.progress(STEPS, Some(STEPS), Some(PROGRESS_REQUEST))
        .await?;
    Ok(Restarted {
        reason: reason.to_owned(),
        requested_by: requested_by.to_owned(),
        applied: true,
        dump: Some(dumped.dir.display().to_string()),
        tables: u32::try_from(dumped.tables.len()).unwrap_or(u32::MAX),
        left_running: u32::try_from(left_running).unwrap_or(u32::MAX),
        exit_status: EXIT_STATUS,
    })
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json;

    use super::*;

    #[test]
    fn the_restart_is_an_internal_admin_action_of_its_own_class() {
        let def = restart();
        assert_eq!(def.name(), RESTART);
        assert_eq!(def.kind(), Kind::Action);
        assert_eq!(def.mode(), Mode::Write);
        assert_eq!(def.access().level, Level::Admin);
        assert!(def.is_internal());
        assert_eq!(def.class(), CLASS);
        assert!(!def.is_resumable());
        let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
        assert!(descriptor.get("ui").is_none_or(Value::is_null));
        assert_eq!(descriptor["params"]["required"], json!(["reason"]));
        assert_eq!(
            descriptor["params"]["properties"]["reason"]["maxLength"],
            json!(200)
        );
        assert_eq!(
            descriptor["result"]["properties"]["exitStatus"]["type"],
            json!("integer")
        );
        assert_eq!(STEPS, 3);
        assert_eq!(EXIT_STATUS, 75);
    }
}
