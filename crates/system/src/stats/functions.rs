use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::prelude::*;
use keel_sdk::sea_orm::{ConnectionTrait, DbBackend, QueryResult, Statement, Value as DbValue};

use super::collectors::{Collector, Stats};
use super::{KEYS, SERIES};

pub const ERROR_KEY: &str = "stats.error.key";
pub const ERROR_SUBJECT: &str = "stats.error.subject";
const ERROR_RANGE: &str = "stats.error.range";
const ERROR_POINTS: &str = "stats.error.points";
const MAX_POINTS: usize = 10_000;
const KEY_LENGTH: u32 = 128;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Nothing {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
enum Scope {
    Installation,
    Account,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "lowercase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "lowercase")]
enum Resolution {
    Raw,
    Hour,
    Day,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
#[schemars(crate = "keel_sdk::schemars", rename_all = "camelCase")]
struct KeyInfo {
    key: String,
    unit: String,
    interval_seconds: u64,
    scope: Scope,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Keys {
    keys: Vec<KeyInfo>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct SeriesQuery {
    #[schemars(length(min = 1, max = KEY_LENGTH))]
    key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = KEY_LENGTH))]
    subject: Option<String>,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    resolution: Resolution,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Point {
    at: DateTime<Utc>,
    value: f64,
    min: f64,
    max: f64,
    sum: f64,
    count: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(crate = "keel_sdk::serde")]
#[schemars(crate = "keel_sdk::schemars")]
struct Series {
    key: String,
    unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    resolution: Resolution,
    points: Vec<Point>,
}

pub(super) fn keys(stats: Arc<Stats>) -> FunctionDef {
    function::<Nothing, Keys>(KEYS)
        .kind(Kind::Get)
        .level(Level::User)
        .unowned()
        .handler(move |call: Call<Nothing>| {
            let stats = stats.clone();
            async move {
                let everything = *call.account_scope() == AccountScope::All;
                let keys = stats
                    .collectors()?
                    .iter()
                    .filter(|collector| everything || collector.def.is_per_account())
                    .map(|collector| KeyInfo {
                        key: collector.def.key().to_owned(),
                        unit: collector.def.unit().as_str().to_owned(),
                        interval_seconds: collector.def.interval().as_secs(),
                        scope: scope_of(collector),
                    })
                    .collect();
                Ok(Keys { keys })
            }
        })
}

pub(super) fn series(stats: Arc<Stats>) -> FunctionDef {
    function::<SeriesQuery, Series>(SERIES)
        .kind(Kind::Get)
        .level(Level::User)
        .unowned()
        .handler(move |call: Call<SeriesQuery>| {
            let stats = stats.clone();
            async move {
                let query = call.params();
                let collector = stats
                    .find(&query.key)?
                    .ok_or_else(|| unknown_key(&query.key))?;
                let subject = subject_of(&call, collector).await?;
                if query.to <= query.from {
                    return Err(KeelError::validation(ERROR_RANGE).field("to"));
                }
                let points = points(call.db(), query, &subject).await?;
                Ok(Series {
                    key: query.key.clone(),
                    unit: collector.def.unit().as_str().to_owned(),
                    subject: (!subject.is_empty()).then_some(subject),
                    resolution: query.resolution,
                    points,
                })
            }
        })
}

fn scope_of(collector: &Collector) -> Scope {
    if collector.def.is_per_account() {
        Scope::Account
    } else {
        Scope::Installation
    }
}

fn unknown_key(key: &str) -> KeelError {
    KeelError::not_found(ERROR_KEY).param("key", key)
}

async fn subject_of(call: &Call<SeriesQuery>, collector: &Collector) -> Result<String> {
    let query = call.params();
    if !collector.def.is_per_account() {
        if *call.account_scope() != AccountScope::All {
            return Err(unknown_key(&query.key));
        }
        return match &query.subject {
            None => Ok(String::new()),
            Some(subject) if collector.def.is_per_subject() => Ok(subject.clone()),
            Some(subject) => Err(refused_subject(&query.key, subject)),
        };
    }
    let own = call.principal().account_id;
    let account = match &query.subject {
        None => own,
        Some(subject) => subject
            .parse::<Id>()
            .map_err(|_| refused_subject(&query.key, subject))?,
    };
    if account != own && !call.account_scope().includes(call.db(), account).await? {
        return Err(refused_subject(&query.key, &account.to_string()));
    }
    Ok(account.to_string())
}

fn refused_subject(key: &str, subject: &str) -> KeelError {
    KeelError::not_found(ERROR_SUBJECT)
        .param("key", key)
        .param("subject", subject)
}

async fn points(db: &Db, query: &SeriesQuery, subject: &str) -> Result<Vec<Point>> {
    let limit = i64::try_from(MAX_POINTS + 1).unwrap_or(i64::MAX);
    let statement = match query.resolution {
        Resolution::Raw => Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT at, value AS min, value AS max, value AS sum, 1::BIGINT AS count \
             FROM stats_sample \
             WHERE key = $1 AND subject = $2 AND at >= $3 AND at < $4 \
             ORDER BY at LIMIT $5",
            [
                DbValue::from(query.key.as_str()),
                DbValue::from(subject),
                DbValue::from(query.from),
                DbValue::from(query.to),
                DbValue::from(limit),
            ],
        ),
        Resolution::Hour | Resolution::Day => Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT bucket AS at, min, max, sum, count \
             FROM stats_agg \
             WHERE key = $1 AND subject = $2 AND resolution = $3 AND bucket >= $4 AND bucket < $5 \
             ORDER BY bucket LIMIT $6",
            [
                DbValue::from(query.key.as_str()),
                DbValue::from(subject),
                DbValue::from(resolution_name(query.resolution)),
                DbValue::from(query.from),
                DbValue::from(query.to),
                DbValue::from(limit),
            ],
        ),
    };
    let rows = db.query_all(statement).await?;
    if rows.len() > MAX_POINTS {
        return Err(KeelError::validation(ERROR_POINTS)
            .field("resolution")
            .param("max", MAX_POINTS));
    }
    rows.iter().map(point).collect()
}

fn resolution_name(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Raw => "raw",
        Resolution::Hour => "hour",
        Resolution::Day => "day",
    }
}

fn point(row: &QueryResult) -> Result<Point> {
    let sum: f64 = row.try_get("", "sum")?;
    let count: i64 = row.try_get("", "count")?;
    let count = u64::try_from(count).unwrap_or_default();
    let value = if count == 0 { sum } else { sum / count as f64 };
    Ok(Point {
        at: row.try_get("", "at")?,
        value,
        min: row.try_get("", "min")?,
        max: row.try_get("", "max")?,
        sum,
        count,
    })
}
