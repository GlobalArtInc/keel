use std::collections::BTreeMap;

use keel_sdk::chrono::{DateTime, SecondsFormat, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{DbBackend, EntityTrait, Statement};
use keel_sdk::types::{Filter, FilterOp, FilterValue};

use super::api::{Outcome, Scheduler, Started};
use super::entity::run;
use super::scheduler::{outcome_of, resolve_override};

pub const JOB_LIST: &str = "cron.job.list";
pub const JOB_RUN: &str = "cron.job.run";
pub const RUN_LIST: &str = "cron.run.list";

const LAST_RUNS: &str =
    "SELECT DISTINCT ON (job) * FROM cron_run ORDER BY job, started_at DESC, id DESC";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
pub struct Pick {
    #[schemars(length(min = 1, max = 128))]
    pub id: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct JobRow {
    pub id: String,
    pub module: String,
    pub schedule: String,
    pub effective: String,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_outcome: Option<Outcome>,
    pub next_run_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
pub struct RunRow {
    pub id: String,
    pub job: String,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: Outcome,
    pub error: Option<String>,
}

fn stamp(at: Option<DateTime<Utc>>) -> Option<String> {
    at.map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn column_text(row: &JobRow, key: &str) -> Option<String> {
    match key {
        "id" => Some(row.id.clone()),
        "module" => Some(row.module.clone()),
        "schedule" => Some(row.schedule.clone()),
        "effective" => Some(row.effective.clone()),
        "lastRunAt" => stamp(row.last_run_at),
        "lastOutcome" => row.last_outcome.map(|outcome| outcome.as_str().to_owned()),
        "nextRunAt" => stamp(row.next_run_at),
        _ => None,
    }
}

fn matches(row: &JobRow, filter: &Filter, index: usize) -> Result<bool> {
    let actual = column_text(row, &filter.key);
    Ok(match (&filter.value, filter.op) {
        (FilterValue::Many(wanted), FilterOp::Eq) => {
            actual.is_some_and(|actual| wanted.contains(&actual))
        }
        (FilterValue::Many(wanted), FilterOp::Ne) => {
            !actual.is_some_and(|actual| wanted.contains(&actual))
        }
        (FilterValue::Many(_), _) => {
            return Err(KeelError::validation("keel.error.list.list_value_op")
                .field(format!("filters[{index}].op"))
                .param("field", filter.key.clone()));
        }
        (FilterValue::One(wanted), op) => {
            let Some(actual) = actual else {
                return Ok(op == FilterOp::Ne);
            };
            match op {
                FilterOp::Eq => actual == *wanted,
                FilterOp::Ne => actual != *wanted,
                FilterOp::Gt => actual > *wanted,
                FilterOp::Gte => actual >= *wanted,
                FilterOp::Lt => actual < *wanted,
                FilterOp::Lte => actual <= *wanted,
            }
        }
    })
}

async fn last_runs(db: &Db) -> Result<BTreeMap<String, run::Model>> {
    let rows = run::Entity::find()
        .from_raw_sql(Statement::from_string(DbBackend::Postgres, LAST_RUNS))
        .all(db)
        .await?;
    Ok(rows.into_iter().map(|row| (row.job.clone(), row)).collect())
}

pub(super) async fn job_rows(
    scheduler: &Scheduler,
    db: &Db,
    now: DateTime<Utc>,
) -> Result<Vec<JobRow>> {
    let started = scheduler.core().started()?;
    let overrides = started.overrides().await?;
    let last = last_runs(db).await?;
    let mut rows = Vec::new();
    for job in started.jobs() {
        let effective = resolve_override(&job.contributed, overrides.get(&job.id)).schedule;
        let last = last.get(&job.id);
        rows.push(JobRow {
            id: job.id.clone(),
            module: job.module.clone(),
            schedule: job.contributed.text().to_owned(),
            effective: effective.text().to_owned(),
            last_run_at: last.map(|row| row.started_at),
            last_outcome: last.map(outcome_of).transpose()?,
            next_run_at: effective.next_after(now),
        });
    }
    Ok(rows)
}

struct JobSource;

#[async_trait]
impl ListSource for JobSource {
    type Row = JobRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<JobRow>> {
        let query = call.params();
        let scheduler = call.require::<Scheduler>()?;
        let now = call.require::<dyn Clock>()?.now();
        let search = query.search.to_lowercase();
        let mut rows = Vec::new();
        for row in job_rows(&scheduler, call.db(), now).await? {
            let searched = search.is_empty()
                || spec.search.iter().any(|key| {
                    column_text(&row, key).is_some_and(|text| text.to_lowercase().contains(&search))
                });
            if !searched {
                continue;
            }
            let mut kept = true;
            for (index, filter) in query.filters.iter().enumerate() {
                if !matches(&row, filter, index)? {
                    kept = false;
                    break;
                }
            }
            if kept {
                rows.push(row);
            }
        }
        if let Some(sort) = &query.sort {
            rows.sort_by(|a, b| {
                let order = column_text(a, &sort.key).cmp(&column_text(b, &sort.key));
                match sort.dir {
                    SortDir::Asc => order,
                    SortDir::Desc => order.reverse(),
                }
                .then_with(|| a.id.cmp(&b.id))
            });
        }
        let total = rows.len() as u64;
        let page_size = query.page_size as usize;
        let start = (query.page as usize - 1).saturating_mul(page_size);
        let data = rows.into_iter().skip(start).take(page_size).collect();
        Ok(Page {
            data,
            total,
            totals: None,
        })
    }
}

fn outcome_tones() -> [(&'static str, Tone); 5] {
    [
        (Outcome::Running.as_str(), Tone::Info),
        (Outcome::Ok.as_str(), Tone::Success),
        (Outcome::Failed.as_str(), Tone::Danger),
        (Outcome::Skipped.as_str(), Tone::Warning),
        (Outcome::Interrupted.as_str(), Tone::Warning),
    ]
}

pub fn job_list() -> FunctionDef {
    let outcomes: Vec<&str> = Outcome::ALL
        .iter()
        .map(|outcome| outcome.as_str())
        .collect();
    function::<ListQuery, Page<JobRow>>(JOB_LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(List::new("cron.job.list.title")
            .search(["id", "module"])
            .column(Column::new("id", ColumnType::Text).sortable().primary())
            .column(Column::new("module", ColumnType::Text).sortable())
            .column(Column::new("schedule", ColumnType::Text))
            .column(Column::new("effective", ColumnType::Text))
            .column(Column::new("lastRunAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("lastOutcome", ColumnType::Status)
                    .map(outcome_tones())
                    .filter_options(outcomes.iter().copied()),
            )
            .column(Column::new("nextRunAt", ColumnType::Datetime).sortable())
            .action(
                Action::new("run", ActionRole::Button)
                    .function(JOB_RUN)
                    .params([("id", "$id")])
                    .icon("play")
                    .confirm(),
            )
            .row_actions(["run"])
            .empty("cron.job.empty", None)
            .invalidate_on(["cron.*"]))
        .default_sort("id", SortDir::Asc)
        .source(JobSource)
}

pub fn job_run() -> FunctionDef {
    function::<Pick, Started>(JOB_RUN)
        .kind(Kind::Action)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .handler(|call: Call<Pick>| async move {
            call.require::<Scheduler>()?
                .run_now(&call.params().id)
                .await
        })
}

fn run_row(row: run::Model) -> Result<RunRow> {
    let outcome = outcome_of(&row)?;
    Ok(RunRow {
        id: row.id,
        job: row.job,
        scheduled_at: row.scheduled_at,
        started_at: row.started_at,
        finished_at: row.finished_at,
        outcome,
        error: row.error,
    })
}

struct RunSource;

#[async_trait]
impl ListSource for RunSource {
    type Row = RunRow;

    async fn fetch(&self, call: &Call<ListQuery>, spec: &ListSpec) -> Result<Page<RunRow>> {
        let page = fetch_page(run::Entity::find(), call, spec).await?;
        Ok(Page {
            data: page
                .data
                .into_iter()
                .map(run_row)
                .collect::<Result<Vec<_>>>()?,
            total: page.total,
            totals: page.totals,
        })
    }
}

pub fn run_list() -> FunctionDef {
    let outcomes: Vec<&str> = Outcome::ALL
        .iter()
        .map(|outcome| outcome.as_str())
        .collect();
    function::<ListQuery, Page<RunRow>>(RUN_LIST)
        .kind(Kind::List)
        .level(Level::Admin)
        .unowned()
        .key("id")
        .ui(List::new("cron.run.list.title")
            .search(["job"])
            .column(Column::new("job", ColumnType::Text).sortable().primary())
            .column(Column::new("scheduledAt", ColumnType::Datetime).sortable())
            .column(Column::new("startedAt", ColumnType::Datetime).sortable())
            .column(Column::new("finishedAt", ColumnType::Datetime).sortable())
            .column(
                Column::new("outcome", ColumnType::Status)
                    .sortable()
                    .map(outcome_tones())
                    .filter_options(outcomes.iter().copied()),
            )
            .column(Column::new("error", ColumnType::Text))
            .empty("cron.run.empty", None)
            .invalidate_on(["cron.*"]))
        .default_sort("startedAt", SortDir::Desc)
        .source(RunSource)
}

#[cfg(test)]
mod tests {
    use keel_sdk::types::Sort;

    use super::*;

    fn row(id: &str, outcome: Option<Outcome>) -> JobRow {
        JobRow {
            id: id.to_owned(),
            module: id
                .split_once('.')
                .map_or(id, |(module, _)| module)
                .to_owned(),
            schedule: "*/5 * * * *".into(),
            effective: "*/5 * * * *".into(),
            last_run_at: outcome.map(|_| "2026-09-15T10:05:00Z".parse().expect("an instant")),
            last_outcome: outcome,
            next_run_at: Some("2026-09-15T10:10:00Z".parse().expect("an instant")),
        }
    }

    #[test]
    fn a_job_row_answers_its_columns_as_text_and_missing_members_as_none() {
        let ran = row("dns.serial_check", Some(Outcome::Ok));
        assert_eq!(column_text(&ran, "id").as_deref(), Some("dns.serial_check"));
        assert_eq!(column_text(&ran, "module").as_deref(), Some("dns"));
        assert_eq!(
            column_text(&ran, "schedule").as_deref(),
            Some("*/5 * * * *")
        );
        assert_eq!(
            column_text(&ran, "effective").as_deref(),
            Some("*/5 * * * *")
        );
        assert_eq!(
            column_text(&ran, "lastRunAt").as_deref(),
            Some("2026-09-15T10:05:00.000Z")
        );
        assert_eq!(column_text(&ran, "lastOutcome").as_deref(), Some("ok"));
        assert_eq!(
            column_text(&ran, "nextRunAt").as_deref(),
            Some("2026-09-15T10:10:00.000Z")
        );
        assert_eq!(column_text(&ran, "other"), None);
        let fresh = row("dns.fresh", None);
        assert_eq!(column_text(&fresh, "lastRunAt"), None);
        assert_eq!(column_text(&fresh, "lastOutcome"), None);
    }

    #[test]
    fn a_filter_compares_the_column_text_and_a_missing_value_matches_ne_alone() {
        let ran = row("dns.serial_check", Some(Outcome::Ok));
        let fresh = row("dns.fresh", None);
        let eq = Filter {
            key: "lastOutcome".into(),
            op: FilterOp::Eq,
            value: FilterValue::One("ok".into()),
        };
        assert!(matches(&ran, &eq, 0).expect("compares"));
        assert!(!matches(&fresh, &eq, 0).expect("compares"));
        let ne = Filter {
            key: "lastOutcome".into(),
            op: FilterOp::Ne,
            value: FilterValue::One("ok".into()),
        };
        assert!(!matches(&ran, &ne, 0).expect("compares"));
        assert!(matches(&fresh, &ne, 0).expect("compares"));
        let many = Filter {
            key: "module".into(),
            op: FilterOp::Eq,
            value: FilterValue::Many(vec!["dns".into(), "panel".into()]),
        };
        assert!(matches(&ran, &many, 0).expect("compares"));
        let not_many = Filter {
            key: "module".into(),
            op: FilterOp::Ne,
            value: FilterValue::Many(vec!["dns".into()]),
        };
        assert!(!matches(&ran, &not_many, 0).expect("compares"));
        let bad = Filter {
            key: "module".into(),
            op: FilterOp::Gt,
            value: FilterValue::Many(vec!["dns".into()]),
        };
        let error = matches(&ran, &bad, 2).expect_err("a list needs eq or ne");
        assert_eq!(error.field_path(), Some("filters[2].op"));
        let after = Filter {
            key: "id".into(),
            op: FilterOp::Gt,
            value: FilterValue::One("dns.m".into()),
        };
        assert!(matches(&ran, &after, 0).expect("compares"));
        assert!(!matches(&fresh, &after, 0).expect("compares"));
    }

    #[test]
    fn the_three_functions_are_admin_lists_and_an_action_with_the_job_id_as_key() {
        let list = job_list();
        assert_eq!(list.name(), JOB_LIST);
        assert_eq!(list.kind(), Kind::List);
        assert_eq!(list.access().level, Level::Admin);
        assert_eq!(list.key(), Some("id"));
        assert!(list.owned().is_none());
        assert_eq!(
            list.limits().default_sort,
            Some(Sort {
                key: "id".into(),
                dir: SortDir::Asc
            })
        );
        let run = job_run();
        assert_eq!(run.name(), JOB_RUN);
        assert_eq!(run.kind(), Kind::Action);
        assert_eq!(run.mode(), Mode::Write);
        assert_eq!(run.key(), Some("id"));
        let runs = run_list();
        assert_eq!(runs.name(), RUN_LIST);
        assert_eq!(
            runs.limits().default_sort,
            Some(Sort {
                key: "startedAt".into(),
                dir: SortDir::Desc
            })
        );
        for def in [&list, &run, &runs] {
            def.descriptor().expect("the descriptor compiles");
        }
    }
}
