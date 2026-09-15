use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::sea_query::Expr;
use keel_sdk::sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use keel_sdk::serde_json::Map;
use keel_sdk::system::{AdvisoryLock, Caller};
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

use super::api::{CronJob, JobHandler, JobRun, Outcome};
use super::entity::run;
use super::leader;
use super::schedule::{self, Schedule};

pub const TICK: Duration = Duration::from_secs(1);
pub const WAIT_STEP: Duration = Duration::from_millis(50);
pub const OVERRIDES_KEY: &str = "cron.overrides";
pub const ERROR_NOT_RUNNING: &str = "cron.error.not_running";
pub const ERROR_RUN_NOT_FOUND: &str = "cron.error.run_not_found";

pub(super) struct Job {
    pub id: String,
    pub module: String,
    pub contributed: Schedule,
    pub handler: JobHandler,
}

#[derive(Default)]
pub(super) struct Core {
    started: OnceLock<Arc<Started>>,
}

pub(super) struct Started {
    db: Db,
    clock: Arc<dyn Clock>,
    caller: Arc<dyn Caller>,
    settings: Arc<dyn SettingsSource>,
    ctx: Context,
    jobs: Vec<Arc<Job>>,
    running: Mutex<BTreeMap<String, Id>>,
    leading: AtomicBool,
    settled: Notify,
}

pub(super) enum Begin {
    Started(Id),
    Busy,
}

impl Core {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start<'a>(
        &self,
        db: Db,
        clock: Arc<dyn Clock>,
        caller: Arc<dyn Caller>,
        settings: Arc<dyn SettingsSource>,
        ctx: Context,
        contributed: impl IntoIterator<Item = &'a CronJob>,
    ) -> Result<Arc<Started>> {
        let mut jobs = Vec::new();
        for job in contributed {
            jobs.push(Arc::new(Job {
                id: job.id().to_owned(),
                module: job.module().to_owned(),
                contributed: schedule::contributed(job.id(), job.schedule())?,
                handler: job.handler(),
            }));
        }
        let started = Arc::new(Started {
            db,
            clock,
            caller,
            settings,
            ctx,
            jobs,
            running: Mutex::new(BTreeMap::new()),
            leading: AtomicBool::new(false),
            settled: Notify::new(),
        });
        if self.started.set(started.clone()).is_err() {
            tracing::warn!("the cron scheduler was started twice; the first start stays");
        }
        Ok(started)
    }

    pub fn started(&self) -> Result<&Arc<Started>> {
        self.started
            .get()
            .ok_or_else(|| KeelError::unavailable(ERROR_NOT_RUNNING))
    }
}

impl Started {
    pub fn is_leader(&self) -> bool {
        self.leading.load(Ordering::SeqCst)
    }

    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub fn jobs(&self) -> impl Iterator<Item = &Arc<Job>> {
        self.jobs.iter()
    }

    pub fn job(&self, id: &str) -> Option<&Arc<Job>> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn running(&self) -> Vec<String> {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    pub fn is_running(&self, job: &str) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(job)
    }

    pub fn settled(&self) -> Notified<'_> {
        self.settled.notified()
    }

    pub async fn overrides(&self) -> Result<Map<String, Value>> {
        let value = self
            .settings
            .resolve(OVERRIDES_KEY, Id::ROOT_ACCOUNT, &self.db)
            .await?;
        Ok(match value {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        })
    }

    fn reserve(&self, job: &str, run: Id) -> bool {
        let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        if running.contains_key(job) {
            return false;
        }
        running.insert(job.to_owned(), run);
        true
    }

    fn release(&self, job: &str) {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(job);
        self.settled.notify_waiters();
    }

    pub async fn begin(
        self: &Arc<Self>,
        job: Arc<Job>,
        scheduled_at: DateTime<Utc>,
    ) -> Result<Begin> {
        let run = Id::generate();
        if !self.reserve(&job.id, run) {
            return Ok(Begin::Busy);
        }
        if let Err(error) = self
            .record(run, &job.id, scheduled_at, Outcome::Running)
            .await
        {
            self.release(&job.id);
            return Err(error);
        }
        let started = self.clone();
        self.ctx.spawn(async move {
            started.perform(job, run, scheduled_at).await;
        });
        Ok(Begin::Started(run))
    }

    async fn perform(self: Arc<Self>, job: Arc<Job>, run: Id, scheduled_at: DateTime<Utc>) {
        let _release = Release {
            started: &self,
            job: &job.id,
        };
        let handle = JobRun::new(
            &job.id,
            run,
            scheduled_at,
            self.db.clone(),
            self.caller.clone(),
        );
        let (outcome, error) = match (job.handler)(handle).await {
            Ok(()) => (Outcome::Ok, None),
            Err(error) => {
                tracing::warn!(job = job.id, run = %run, error = %error, "the job failed");
                (Outcome::Failed, Some(error.to_string()))
            }
        };
        if let Err(error) = self.finish(run, outcome, error.as_deref()).await {
            tracing::error!(job = job.id, run = %run, error = %error, "the run outcome could not be stored");
        }
    }

    pub async fn skip(&self, job: &Job, scheduled_at: DateTime<Utc>) -> Result<()> {
        let run = Id::generate();
        let now = self.now();
        run::ActiveModel {
            id: Set(run.to_string()),
            job: Set(job.id.clone()),
            scheduled_at: Set(scheduled_at),
            started_at: Set(now),
            finished_at: Set(Some(now)),
            outcome: Set(Outcome::Skipped.as_str().to_owned()),
            error: Set(None),
        }
        .insert(&self.db)
        .await?;
        tracing::warn!(job = job.id, scheduled_at = %scheduled_at, "the previous run is still going; the occurrence is skipped");
        self.settled.notify_waiters();
        Ok(())
    }

    async fn record(
        &self,
        run: Id,
        job: &str,
        scheduled_at: DateTime<Utc>,
        outcome: Outcome,
    ) -> Result<()> {
        run::ActiveModel {
            id: Set(run.to_string()),
            job: Set(job.to_owned()),
            scheduled_at: Set(scheduled_at),
            started_at: Set(self.now()),
            finished_at: Set(None),
            outcome: Set(outcome.as_str().to_owned()),
            error: Set(None),
        }
        .insert(&self.db)
        .await?;
        Ok(())
    }

    async fn finish(&self, run: Id, outcome: Outcome, error: Option<&str>) -> Result<()> {
        run::Entity::update_many()
            .filter(run::Column::Id.eq(run.to_string()))
            .filter(run::Column::FinishedAt.is_null())
            .col_expr(run::Column::FinishedAt, Expr::value(Some(self.now())))
            .col_expr(run::Column::Outcome, Expr::value(outcome.as_str()))
            .col_expr(run::Column::Error, Expr::value(error.map(str::to_owned)))
            .exec(&self.db)
            .await?;
        Ok(())
    }

    pub async fn final_outcome(&self, run: Id) -> Result<Option<Outcome>> {
        let row = run::Entity::find_by_id(run.to_string())
            .one(&self.db)
            .await?
            .ok_or_else(|| {
                KeelError::not_found(ERROR_RUN_NOT_FOUND).param("run", run.to_string())
            })?;
        let outcome = outcome_of(&row)?;
        Ok(outcome.is_final().then_some(outcome))
    }

    async fn recover(&self) -> Result<u64> {
        let interrupted = run::Entity::update_many()
            .filter(run::Column::FinishedAt.is_null())
            .col_expr(run::Column::FinishedAt, Expr::value(Some(self.now())))
            .col_expr(
                run::Column::Outcome,
                Expr::value(Outcome::Interrupted.as_str()),
            )
            .exec(&self.db)
            .await?;
        Ok(interrupted.rows_affected)
    }

    pub async fn stop(&self) -> Result<()> {
        let in_flight: Vec<String> = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(ToString::to_string)
            .collect();
        if in_flight.is_empty() {
            return Ok(());
        }
        run::Entity::update_many()
            .filter(run::Column::Id.is_in(in_flight))
            .filter(run::Column::FinishedAt.is_null())
            .col_expr(run::Column::FinishedAt, Expr::value(Some(self.now())))
            .col_expr(
                run::Column::Outcome,
                Expr::value(Outcome::Interrupted.as_str()),
            )
            .exec(&self.db)
            .await?;
        Ok(())
    }
}

struct Release<'a> {
    started: &'a Started,
    job: &'a str,
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.started.release(self.job);
    }
}

pub fn outcome_of(row: &run::Model) -> Result<Outcome> {
    row.outcome.parse().map_err(|()| {
        KeelError::internal(super::api::ERROR_OUTCOME)
            .param("run", row.id.as_str())
            .param("outcome", row.outcome.as_str())
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub schedule: Schedule,
    pub rejected: Option<String>,
}

pub fn resolve_override(contributed: &Schedule, wanted: Option<&Value>) -> Resolved {
    let Some(wanted) = wanted else {
        return Resolved {
            schedule: contributed.clone(),
            rejected: None,
        };
    };
    let text = match wanted {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match Schedule::parse(&text) {
        Ok(schedule) => Resolved {
            schedule,
            rejected: None,
        },
        Err(_) => Resolved {
            schedule: contributed.clone(),
            rejected: Some(text),
        },
    }
}

pub(super) struct Plan {
    last_tick: DateTime<Utc>,
    entries: Vec<Entry>,
}

struct Entry {
    id: String,
    contributed: Schedule,
    effective: Schedule,
    next: Option<DateTime<Utc>>,
    warned: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Due {
    pub index: usize,
    pub scheduled_at: DateTime<Utc>,
}

impl Plan {
    pub fn new<'a>(
        jobs: impl IntoIterator<Item = (&'a str, &'a Schedule)>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            last_tick: now,
            entries: jobs
                .into_iter()
                .map(|(id, contributed)| Entry {
                    id: id.to_owned(),
                    contributed: contributed.clone(),
                    effective: contributed.clone(),
                    next: contributed.next_after(now),
                    warned: None,
                })
                .collect(),
        }
    }

    pub fn apply(&mut self, overrides: &Map<String, Value>) -> Vec<(String, String)> {
        let mut rejected = Vec::new();
        for entry in &mut self.entries {
            let resolved = resolve_override(&entry.contributed, overrides.get(&entry.id));
            match resolved.rejected {
                Some(text) => {
                    if entry.warned.as_deref() != Some(text.as_str()) {
                        rejected.push((entry.id.clone(), text.clone()));
                        entry.warned = Some(text);
                    }
                }
                None => entry.warned = None,
            }
            if resolved.schedule != entry.effective {
                entry.next = resolved.schedule.next_after(self.last_tick);
                entry.effective = resolved.schedule;
            }
        }
        rejected
    }

    pub fn due(&mut self, now: DateTime<Utc>) -> Vec<Due> {
        if now < self.last_tick {
            for entry in &mut self.entries {
                entry.next = entry.effective.next_after(now);
            }
        }
        let mut due = Vec::new();
        for (index, entry) in self.entries.iter_mut().enumerate() {
            if let Some(next) = entry.next
                && now >= next
            {
                due.push(Due {
                    index,
                    scheduled_at: next,
                });
                entry.next = entry.effective.next_after(now);
            }
        }
        self.last_tick = now;
        due
    }

    #[cfg(test)]
    pub fn effective(&self, index: usize) -> Option<&Schedule> {
        self.entries.get(index).map(|entry| &entry.effective)
    }

    #[cfg(test)]
    pub fn next(&self, index: usize) -> Option<DateTime<Utc>> {
        self.entries.get(index).and_then(|entry| entry.next)
    }
}

pub(super) async fn lead(started: Arc<Started>, lock: Arc<dyn AdvisoryLock>) {
    let _guard = match leader::hold(lock.as_ref()).await {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(error = %error, "the cron leader could not take its lock");
            return;
        }
    };
    tracing::info!(jobs = started.jobs.len(), "the cron leader holds the lock");
    match started.recover().await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(
            count,
            "runs left unfinished by an earlier leader are interrupted"
        ),
        Err(error) => tracing::error!(error = %error, "the cron leader could not recover the runs"),
    }
    started.leading.store(true, Ordering::SeqCst);
    let mut plan = Plan::new(
        started
            .jobs
            .iter()
            .map(|job| (job.id.as_str(), &job.contributed)),
        started.now(),
    );
    loop {
        if let Err(error) = tick(&started, &mut plan).await {
            tracing::error!(error = %error, "the cron tick failed");
        }
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(started: &Arc<Started>, plan: &mut Plan) -> Result<()> {
    let overrides = started.overrides().await?;
    for (job, text) in plan.apply(&overrides) {
        tracing::warn!(
            job,
            schedule = text,
            "the override does not parse; the contributed schedule stands"
        );
    }
    let now = started.now();
    for due in plan.due(now) {
        let Some(job) = started.jobs.get(due.index) else {
            continue;
        };
        if started.is_running(&job.id) {
            started.skip(job, due.scheduled_at).await?;
            continue;
        }
        if let Begin::Busy = started.begin(job.clone(), due.scheduled_at).await? {
            started.skip(job, due.scheduled_at).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::json;

    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        text.parse().expect("an instant")
    }

    fn every(text: &str) -> Schedule {
        Schedule::parse(text).expect("parses")
    }

    #[test]
    fn a_job_is_due_once_when_the_clock_passes_its_next_occurrence_and_then_waits_for_the_next() {
        let five = every("*/5 * * * *");
        let hourly = every("0 * * * *");
        let mut plan = Plan::new(
            [("a.five", &five), ("a.hourly", &hourly)],
            at("2026-09-15T10:00:00Z"),
        );
        assert_eq!(plan.next(0), Some(at("2026-09-15T10:05:00Z")));
        assert_eq!(plan.next(1), Some(at("2026-09-15T11:00:00Z")));
        assert!(plan.due(at("2026-09-15T10:04:59Z")).is_empty());
        assert_eq!(
            plan.due(at("2026-09-15T10:05:00Z")),
            [Due {
                index: 0,
                scheduled_at: at("2026-09-15T10:05:00Z")
            }]
        );
        assert!(plan.due(at("2026-09-15T10:05:01Z")).is_empty());
        assert_eq!(plan.next(0), Some(at("2026-09-15T10:10:00Z")));
        let jumped = plan.due(at("2026-09-15T11:27:00Z"));
        assert_eq!(
            jumped,
            [
                Due {
                    index: 0,
                    scheduled_at: at("2026-09-15T10:10:00Z")
                },
                Due {
                    index: 1,
                    scheduled_at: at("2026-09-15T11:00:00Z")
                }
            ]
        );
        assert_eq!(plan.next(0), Some(at("2026-09-15T11:30:00Z")));
        assert_eq!(plan.next(1), Some(at("2026-09-15T12:00:00Z")));
        assert!(plan.due(at("2026-09-15T09:00:00Z")).is_empty());
        assert_eq!(plan.next(0), Some(at("2026-09-15T09:05:00Z")));
        assert!(plan.next(2).is_none());
        assert!(plan.effective(2).is_none());
    }

    #[test]
    fn an_override_replaces_the_schedule_from_the_last_tick_and_a_bad_one_is_reported_once() {
        let five = every("*/5 * * * *");
        let mut plan = Plan::new([("a.five", &five)], at("2026-09-15T10:00:00Z"));
        let mut overrides = Map::new();
        overrides.insert("a.five".into(), json!("*/2 * * * *"));
        assert!(plan.apply(&overrides).is_empty());
        assert_eq!(plan.effective(0).map(Schedule::text), Some("*/2 * * * *"));
        assert_eq!(plan.next(0), Some(at("2026-09-15T10:02:00Z")));
        assert_eq!(
            plan.due(at("2026-09-15T10:02:00Z")),
            [Due {
                index: 0,
                scheduled_at: at("2026-09-15T10:02:00Z")
            }]
        );
        overrides.insert("a.five".into(), json!("every day"));
        assert_eq!(
            plan.apply(&overrides),
            [("a.five".to_owned(), "every day".to_owned())]
        );
        assert!(plan.apply(&overrides).is_empty());
        assert_eq!(plan.effective(0).map(Schedule::text), Some("*/5 * * * *"));
        assert_eq!(plan.next(0), Some(at("2026-09-15T10:05:00Z")));
        overrides.insert("a.five".into(), json!(7));
        assert_eq!(
            plan.apply(&overrides),
            [("a.five".to_owned(), "7".to_owned())]
        );
        overrides.remove("a.five");
        assert!(plan.apply(&overrides).is_empty());
        assert_eq!(plan.effective(0).map(Schedule::text), Some("*/5 * * * *"));
        overrides.insert("a.five".into(), json!("every day"));
        assert_eq!(
            plan.apply(&overrides),
            [("a.five".to_owned(), "every day".to_owned())]
        );
        overrides.insert("a.other".into(), json!("* * * * *"));
        assert!(plan.apply(&overrides).is_empty());
    }

    #[test]
    fn an_override_resolves_to_itself_when_it_parses_and_to_the_contributed_schedule_otherwise() {
        let five = every("*/5 * * * *");
        assert_eq!(
            resolve_override(&five, None),
            Resolved {
                schedule: five.clone(),
                rejected: None
            }
        );
        assert_eq!(
            resolve_override(&five, Some(&json!("0 * * * *"))),
            Resolved {
                schedule: every("0 * * * *"),
                rejected: None
            }
        );
        assert_eq!(
            resolve_override(&five, Some(&json!("soon"))),
            Resolved {
                schedule: five.clone(),
                rejected: Some("soon".into())
            }
        );
        assert_eq!(
            resolve_override(&five, Some(&json!(null))),
            Resolved {
                schedule: five,
                rejected: Some("null".into())
            }
        );
    }

    #[test]
    fn an_unknown_stored_outcome_is_an_internal_error_naming_the_run() {
        let row = run::Model {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9Z3".into(),
            job: "a.five".into(),
            scheduled_at: Utc::now(),
            started_at: Utc::now(),
            finished_at: None,
            outcome: "paused".into(),
            error: None,
        };
        let error = outcome_of(&row).expect_err("paused is not an outcome");
        assert_eq!(error.key(), super::super::api::ERROR_OUTCOME);
        assert_eq!(error.params()["outcome"], json!("paused"));
        let core = Core::new();
        let Err(not_started) = core.started() else {
            panic!("a core answers nothing before its start");
        };
        assert_eq!(not_started.key(), ERROR_NOT_RUNNING);
        assert_eq!(TICK.as_secs(), 1);
    }
}
