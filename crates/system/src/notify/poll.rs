use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use super::super::cron::api::{CronJob, JobRun, JobRunExt};
use super::api::{ProblemSource, Problems};
use super::problems::open_of_source;

pub const JOB_ID: &str = "notify.problems.poll";
pub const SCHEDULE: &str = "* * * * *";

pub type Sources = Arc<OnceLock<Vec<ProblemSource>>>;

pub fn job(sources: Sources) -> CronJob {
    CronJob::new(JOB_ID, SCHEDULE, move |run| {
        let sources = sources.clone();
        async move {
            let Some(sources) = sources.get() else {
                return Ok(());
            };
            for source in sources {
                poll_source(&run, source).await;
            }
            Ok(())
        }
    })
}

pub(super) async fn poll_source(run: &JobRun, source: &ProblemSource) {
    let reported = match source.poll(run.db()).await {
        Ok(reported) => reported,
        Err(error) => {
            tracing::warn!(source = source.id(), error = %error, "the problem source failed; its problems stand");
            return;
        }
    };
    let caller = run.caller();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for raise in reported {
        seen.insert((raise.key.clone(), raise.subject.clone()));
        let raise = raise.source(source.id());
        if let Err(error) = Problems::raise(caller.as_ref(), raise).await {
            tracing::error!(source = source.id(), error = %error, "a problem could not be raised");
        }
    }
    let open = match open_of_source(&run.db(), source.id()).await {
        Ok(open) => open,
        Err(error) => {
            tracing::error!(source = source.id(), error = %error, "the open problems of the source could not be read");
            return;
        }
    };
    for problem in open {
        if seen.contains(&(problem.key.clone(), problem.subject.clone())) {
            continue;
        }
        if let Err(error) = Problems::clear(caller.as_ref(), &problem.key, &problem.subject).await {
            tracing::error!(source = source.id(), key = problem.key, subject = problem.subject, error = %error, "a problem could not be cleared");
        }
    }
}
