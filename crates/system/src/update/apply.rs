use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::Serialize;
use keel_sdk::types::ScreenResult;
use semver::Version;

use super::super::tasks::api::Drain;
use super::channel::Channel;
use super::history::{self, Run, RunKind};
use super::status::{self, Blocker, Nothing, Site};
use super::{APPLY, ROLLBACK, dump, request};

const CLASS: &str = "update";
const ERROR_INLINE: &str = "update.error.inline";
const ERROR_NOT_STARTED: &str = "update.error.not_started";
pub const ERROR_UP_TO_DATE: &str = "update.error.up_to_date";
pub const ERROR_MIN_UPGRADE_FROM: &str = "update.error.min_upgrade_from";
pub const ERROR_CONTRACT: &str = "update.error.contract";
pub const ERROR_CONTRACT_UNKNOWN: &str = "update.error.contract_unknown";
pub const ERROR_NO_PREVIOUS: &str = "update.error.no_previous";
const PROGRESS_DRAIN: &str = "update.progress.drain";
const PROGRESS_DUMP: &str = "update.progress.dump";
const PROGRESS_REQUEST: &str = "update.progress.request";
const STEPS: u64 = 3;
const RESUME_AFTER: Duration = Duration::from_secs(15 * 60);

pub(super) type Pool = Arc<OnceLock<Db>>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub(super) struct Requested {
    pub(super) kind: RunKind,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) channel: Channel,
    pub(super) requested_by: String,
    pub(super) dump: String,
    pub(super) tables: u32,
    pub(super) request: String,
    pub(super) left_running: u32,
}

pub(super) fn apply(pool: Pool) -> FunctionDef {
    function::<Nothing, Requested>(APPLY)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .class(CLASS)
        .ui(ActionScreen::new("update.apply.title").result(ScreenResult::Task))
        .handler(move |call: Call<Nothing>| {
            let pool = pool.clone();
            async move { run(&call, &pool, RunKind::Apply).await }
        })
}

pub(super) fn rollback(pool: Pool) -> FunctionDef {
    function::<Nothing, Requested>(ROLLBACK)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .class(CLASS)
        .ui(ActionScreen::new("update.rollback.title").result(ScreenResult::Task))
        .handler(move |call: Call<Nothing>| {
            let pool = pool.clone();
            async move { run(&call, &pool, RunKind::Rollback).await }
        })
}

async fn run(call: &Call<Nothing>, pool: &Pool, kind: RunKind) -> Result<Requested> {
    let task = call
        .task()
        .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
    let pool = pool
        .get()
        .ok_or_else(|| KeelError::internal(ERROR_NOT_STARTED))?;
    let site = status::site(call).await?;
    let to = match kind {
        RunKind::Apply => apply_target(&site, call.db()).await?,
        RunKind::Rollback => rollback_target(&site, call.db()).await?,
    };
    let drain = call.require::<Drain>()?;
    let data_dir = drain.data_dir()?;
    let now = call.require::<dyn Clock>()?.now();
    task.progress(0, Some(STEPS), Some(PROGRESS_DRAIN)).await?;
    task.log(&format!(
        "{kind} {} -> {} from {} requested by {}",
        site.running,
        to,
        site.channel,
        call.principal().login
    ))
    .await?;
    let quiesced = drain.quiesce(task.id()).await?;
    let left_running = quiesced.still_running.len();
    if left_running == 0 {
        task.log("the task queue is drained; no other job runs")
            .await?;
    } else {
        task.log(&format!(
            "the task queue is drained; {left_running} job(s) did not stop within the grace and will be interrupted by the restart"
        ))
        .await?;
    }
    let plan = Plan {
        kind,
        to: &to,
        data_dir: &data_dir,
        now,
        left_running,
    };
    let outcome = requested(call, pool, &site, &plan).await;
    match &outcome {
        Ok(_) => {
            let drain = drain.clone();
            tokio::spawn(async move {
                tokio::time::sleep(RESUME_AFTER).await;
                if drain.is_quiesced().is_ok_and(|quiesced| quiesced) {
                    tracing::warn!(
                        "the update request was written {}s ago and the service still runs; the task queue resumes",
                        RESUME_AFTER.as_secs()
                    );
                    if let Err(error) = drain.resume() {
                        tracing::error!(error = %error, "the task queue could not resume");
                    }
                }
            });
        }
        Err(_) => {
            if let Err(error) = drain.resume() {
                tracing::error!(error = %error, "the task queue could not resume after a refused update");
            }
        }
    }
    outcome
}

struct Plan<'a> {
    kind: RunKind,
    to: &'a Version,
    data_dir: &'a Path,
    now: DateTime<Utc>,
    left_running: usize,
}

async fn requested(
    call: &Call<Nothing>,
    pool: &Db,
    site: &Site,
    plan: &Plan<'_>,
) -> Result<Requested> {
    let task = call
        .task()
        .ok_or_else(|| KeelError::internal(ERROR_INLINE))?;
    let Plan {
        kind,
        to,
        data_dir,
        now,
        left_running,
    } = *plan;
    task.progress(1, Some(STEPS), Some(PROGRESS_DUMP)).await?;
    let running = site.running.to_string();
    let dir = dump::dir_of(data_dir, &running, now);
    let reason = format!("{kind} {running} -> {to}");
    let dumped = dump::write(pool, &dir, &running, &reason, now).await?;
    task.log(&format!(
        "dumped {} table(s) of {} into {}",
        dumped.tables.len(),
        dumped.schema,
        dumped.dir.display()
    ))
    .await?;
    task.progress(2, Some(STEPS), Some(PROGRESS_REQUEST))
        .await?;
    let principal = call.principal();
    let written = request::write(
        data_dir,
        &request::Request {
            version: to.clone(),
            channel: site.channel,
            requested_by: principal.login.clone(),
        },
    )
    .await?;
    let dump_dir = dumped.dir.display().to_string();
    history::record(
        call.tx()?,
        Run {
            kind,
            from: &site.running,
            to,
            channel: site.channel,
            principal,
            request_id: call.request_id(),
            dump_dir: &dump_dir,
            tables: dumped.tables.len(),
            at: now,
        },
    )
    .await?;
    task.log(&format!(
        "the request for {to} is written to {}; the root unit takes over from here and restarts the service",
        written.display()
    ))
    .await?;
    task.progress(STEPS, Some(STEPS), Some(PROGRESS_REQUEST))
        .await?;
    Ok(Requested {
        kind,
        from: running,
        to: to.to_string(),
        channel: site.channel,
        requested_by: principal.login.clone(),
        dump: dump_dir,
        tables: u32::try_from(dumped.tables.len()).unwrap_or(u32::MAX),
        request: written.display().to_string(),
        left_running: u32::try_from(left_running).unwrap_or(u32::MAX),
    })
}

async fn apply_target(site: &Site, db: &Db) -> Result<Version> {
    let status = status::status(site, db).await?;
    if status.up_to_date {
        return Err(KeelError::conflict(ERROR_UP_TO_DATE)
            .param("running", status.running)
            .param("available", status.available));
    }
    match status.blocked_by {
        Some(Blocker::MinUpgradeFrom) => {
            return Err(KeelError::conflict(ERROR_MIN_UPGRADE_FROM)
                .param("running", status.running)
                .param("target", status.available)
                .param("minUpgradeFrom", status.min_upgrade_from));
        }
        Some(Blocker::Contract) => {
            return Err(across(
                &site.running,
                status.running_contract,
                &status.available,
                status.contract,
            ));
        }
        None => {}
    }
    Version::parse(&status.available).map_err(|error| {
        KeelError::internal(status::ERROR_VERSION)
            .param("version", status.available.as_str())
            .source(error)
    })
}

async fn rollback_target(site: &Site, db: &Db) -> Result<Version> {
    let previous = history::previous_of(db, &site.running)
        .await?
        .ok_or_else(|| {
            KeelError::conflict(ERROR_NO_PREVIOUS).param("running", site.running.to_string())
        })?;
    let (head, _) = site.source.head(site.channel).await?;
    if previous < head.min_upgrade_from {
        return Err(KeelError::conflict(ERROR_MIN_UPGRADE_FROM)
            .param("running", site.running.to_string())
            .param("target", previous.to_string())
            .param("minUpgradeFrom", head.min_upgrade_from.to_string()));
    }
    let running_contract = contract_of(site, &site.running).await?;
    let previous_contract = contract_of(site, &previous).await?;
    if running_contract != previous_contract {
        return Err(across(
            &site.running,
            Some(running_contract),
            &previous.to_string(),
            previous_contract,
        ));
    }
    Ok(previous)
}

async fn contract_of(site: &Site, version: &Version) -> Result<u32> {
    site.source
        .manifest(version)
        .await?
        .map(|manifest| manifest.contract)
        .ok_or_else(|| {
            KeelError::conflict(ERROR_CONTRACT_UNKNOWN).param("version", version.to_string())
        })
}

fn across(
    running: &Version,
    running_contract: Option<u32>,
    target: &str,
    target_contract: u32,
) -> KeelError {
    KeelError::conflict(ERROR_CONTRACT)
        .param("running", running.to_string())
        .param("runningContract", running_contract)
        .param("target", target)
        .param("targetContract", target_contract)
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::{self, json};

    use super::*;

    #[test]
    fn the_two_actions_are_admin_tasks_of_one_class() {
        for (def, name, title) in [
            (apply(Pool::default()), APPLY, "update.apply.title"),
            (rollback(Pool::default()), ROLLBACK, "update.rollback.title"),
        ] {
            assert_eq!(def.name(), name);
            assert_eq!(def.kind(), Kind::Action);
            assert_eq!(def.mode(), Mode::Write);
            assert_eq!(def.access().level, Level::Admin);
            assert_eq!(def.class(), CLASS);
            assert!(!def.is_resumable());
            let descriptor = serde_json::to_value(def.descriptor().expect("compiles")).unwrap();
            assert_eq!(
                descriptor["ui"],
                json!({ "title": title, "result": "task" })
            );
            assert_eq!(
                descriptor["result"]["properties"]["kind"]["$ref"],
                json!("#/$defs/RunKind")
            );
        }
        assert_eq!(RESUME_AFTER.as_secs(), 900);
        assert_eq!(STEPS, 3);
    }

    #[test]
    fn a_contract_refusal_names_both_sides() {
        let error = across(&Version::new(0, 2, 0), Some(1), "1.0.0", 2);
        assert_eq!(error.key(), ERROR_CONTRACT);
        assert_eq!(error.params()["running"], json!("0.2.0"));
        assert_eq!(error.params()["runningContract"], json!(1));
        assert_eq!(error.params()["target"], json!("1.0.0"));
        assert_eq!(error.params()["targetContract"], json!(2));
        assert_eq!(
            across(&Version::new(0, 2, 0), None, "1.0.0", 2).params()["runningContract"],
            Value::Null
        );
    }
}
