use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::schemars::JsonSchema;
use keel_sdk::serde::{Deserialize, Serialize};
use keel_sdk::serde_json::Value;
use keel_sdk::system::{Caller, CallerOptions};
use keel_sdk::types::{Scope, Via};
use keel_sdk::{Db, Id, KeelError, Level, Principal, Result};

#[cfg(feature = "plugins")]
use super::schedule::Schedule;
use super::scheduler::{Begin, Core};

pub const OWNER_LOGIN: &str = "owner";
pub const ERROR_UNKNOWN_JOB: &str = "cron.error.unknown_job";
pub const ERROR_RUNNING: &str = "cron.error.running";
pub const ERROR_NOT_LEADER: &str = "cron.error.not_leader";
pub const ERROR_OUTCOME: &str = "cron.error.outcome";

pub type JobFuture = Pin<Box<dyn Future<Output = Result<()>> + Send + 'static>>;
pub type JobHandler = Arc<dyn Fn(JobRun) -> JobFuture + Send + Sync>;

#[cfg(feature = "plugins")]
pub(crate) fn schedule_fault(text: &str) -> Option<String> {
    Schedule::parse(text).err().map(|error| error.reason)
}

pub struct CronJob {
    id: String,
    schedule: String,
    handler: JobHandler,
}

impl CronJob {
    pub fn new<F, Fut>(id: &str, schedule: &str, handler: F) -> Self
    where
        F: Fn(JobRun) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            id: id.to_owned(),
            schedule: schedule.to_owned(),
            handler: Arc::new(move |run| Box::pin(handler(run))),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn schedule(&self) -> &str {
        &self.schedule
    }

    pub fn module(&self) -> &str {
        self.id
            .split_once('.')
            .map_or(&self.id, |(module, _)| module)
    }

    pub(super) fn handler(&self) -> JobHandler {
        self.handler.clone()
    }
}

impl fmt::Debug for CronJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CronJob")
            .field("id", &self.id)
            .field("schedule", &self.schedule)
            .finish()
    }
}

#[derive(Clone)]
pub struct JobRun {
    job: Arc<str>,
    run: Id,
    scheduled_at: DateTime<Utc>,
    db: Db,
    caller: Arc<dyn Caller>,
}

impl JobRun {
    pub(super) fn new(
        job: &str,
        run: Id,
        scheduled_at: DateTime<Utc>,
        db: Db,
        caller: Arc<dyn Caller>,
    ) -> Self {
        Self {
            job: Arc::from(job),
            run,
            scheduled_at,
            db,
            caller,
        }
    }

    pub fn job(&self) -> &str {
        &self.job
    }

    pub fn id(&self) -> Id {
        self.run
    }

    pub fn scheduled_at(&self) -> DateTime<Utc> {
        self.scheduled_at
    }

    pub fn request_id(&self) -> String {
        format!("cron-{}", self.run.to_string().to_lowercase())
    }

    pub async fn invoke(&self, func: &str, params: Value) -> Result<Value> {
        self.caller
            .call(
                owner(),
                func,
                params,
                CallerOptions {
                    request_id: Some(self.request_id()),
                    task: None,
                },
            )
            .await
    }
}

impl fmt::Debug for JobRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobRun")
            .field("job", &self.job)
            .field("run", &self.run)
            .field("scheduled_at", &self.scheduled_at)
            .finish()
    }
}

#[doc(hidden)]
pub trait JobRunExt {
    fn db(&self) -> Db;
    fn caller(&self) -> Arc<dyn Caller>;
}

impl JobRunExt for JobRun {
    fn db(&self) -> Db {
        self.db.clone()
    }

    fn caller(&self) -> Arc<dyn Caller> {
        self.caller.clone()
    }
}

pub fn owner() -> Principal {
    Principal {
        id: Id::ROOT_ACCOUNT,
        login: OWNER_LOGIN.to_owned(),
        level: Level::Owner,
        account_id: Id::ROOT_ACCOUNT,
        role: OWNER_LOGIN.to_owned(),
        actor: None,
        scope: Scope::Full,
        via: Via::Internal,
        external_id: None,
        origin: None,
        lang: "en".to_owned(),
        grants: BTreeMap::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
pub enum Outcome {
    Running,
    Ok,
    Failed,
    Skipped,
    Interrupted,
}

impl Outcome {
    pub const ALL: [Outcome; 5] = [
        Outcome::Running,
        Outcome::Ok,
        Outcome::Failed,
        Outcome::Skipped,
        Outcome::Interrupted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Running => "running",
            Outcome::Ok => "ok",
            Outcome::Failed => "failed",
            Outcome::Skipped => "skipped",
            Outcome::Interrupted => "interrupted",
        }
    }

    pub fn is_final(self) -> bool {
        self != Outcome::Running
    }
}

impl FromStr for Outcome {
    type Err = ();

    fn from_str(text: &str) -> std::result::Result<Self, ()> {
        Outcome::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == text)
            .ok_or(())
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct Started {
    pub id: String,
    pub run: Id,
    pub scheduled_at: DateTime<Utc>,
}

pub struct Scheduler {
    core: Arc<Core>,
}

impl Scheduler {
    pub(super) fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    pub(super) fn core(&self) -> &Arc<Core> {
        &self.core
    }

    pub fn is_leader(&self) -> bool {
        self.core.started().is_ok_and(|started| started.is_leader())
    }

    pub fn jobs(&self) -> Vec<String> {
        self.core.started().map_or_else(
            |_| Vec::new(),
            |started| started.jobs().map(|job| job.id.clone()).collect(),
        )
    }

    pub fn running(&self) -> Vec<String> {
        self.core
            .started()
            .map_or_else(|_| Vec::new(), |started| started.running())
    }

    pub async fn run_now(&self, job: &str) -> Result<Started> {
        let started = self.core.started()?;
        let job = started
            .job(job)
            .ok_or_else(|| KeelError::not_found(ERROR_UNKNOWN_JOB).param("id", job))?;
        if !started.is_leader() {
            return Err(KeelError::unavailable(ERROR_NOT_LEADER).param("id", job.id.as_str()));
        }
        let scheduled_at = started.now();
        match started.begin(job.clone(), scheduled_at).await? {
            Begin::Started(run) => Ok(Started {
                id: job.id.clone(),
                run,
                scheduled_at,
            }),
            Begin::Busy => Err(KeelError::conflict(ERROR_RUNNING).param("id", job.id.as_str())),
        }
    }

    pub async fn wait(&self, run: Id) -> Result<Outcome> {
        let started = self.core.started()?;
        loop {
            let settled = started.settled();
            if let Some(outcome) = started.final_outcome(run).await? {
                return Ok(outcome);
            }
            let _ = tokio::time::timeout(super::scheduler::WAIT_STEP, settled).await;
        }
    }
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scheduler")
            .field("leader", &self.is_leader())
            .field("jobs", &self.jobs())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    #[test]
    fn a_job_carries_its_id_schedule_and_module_and_hides_its_handler_from_debug() {
        let job = CronJob::new("dns.serial_check", "0 */6 * * *", |_run| async { Ok(()) });
        assert_eq!(job.id(), "dns.serial_check");
        assert_eq!(job.schedule(), "0 */6 * * *");
        assert_eq!(job.module(), "dns");
        assert_eq!(
            format!("{job:?}"),
            "CronJob { id: \"dns.serial_check\", schedule: \"0 */6 * * *\" }"
        );
        assert_eq!(
            CronJob::new("flat", "* * * * *", |_| async { Ok(()) }).module(),
            "flat"
        );
    }

    #[test]
    fn the_owner_runs_jobs_internally_from_the_root_account() {
        let owner = owner();
        assert_eq!(owner.id, Id::ROOT_ACCOUNT);
        assert_eq!(owner.account_id, Id::ROOT_ACCOUNT);
        assert_eq!(owner.login, OWNER_LOGIN);
        assert_eq!(owner.level, Level::Owner);
        assert_eq!(owner.via, Via::Internal);
        assert_eq!(owner.scope, Scope::Full);
        assert!(owner.actor.is_none());
    }

    #[test]
    fn an_outcome_round_trips_through_its_text_and_running_is_the_only_open_one() {
        for outcome in Outcome::ALL {
            assert_eq!(outcome.as_str().parse::<Outcome>(), Ok(outcome));
            assert_eq!(json!(outcome), json!(outcome.as_str()));
            assert_eq!(outcome.to_string(), outcome.as_str());
            assert_eq!(outcome.is_final(), outcome != Outcome::Running);
        }
        assert_eq!("paused".parse::<Outcome>(), Err(()));
    }
}
