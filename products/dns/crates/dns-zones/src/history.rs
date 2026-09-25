use std::collections::VecDeque;
use std::time::Duration;

use keel::prelude::*;
use keel::sdk::FunctionBuilder;
use keel::sdk::chrono::{DateTime, Utc};
use keel::sdk::futures::stream;
use keel::sdk::sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement};

use crate::zone;

pub(crate) const LOG: &str = "dns.zone.log";
const READ: &str = "dns.zone.read";
const FEATURE: &str = "dns.zones";
const BACKLOG: i64 = 200;
const KEEP: i64 = 500;
const POLL: Duration = Duration::from_secs(1);
const UNREADABLE: &str = "dns.zone.error.log";
const INFO: &str = "info";
const ERROR: &str = "error";

const INSERT: &str = "INSERT INTO dns_zone_log (zone_id, at, level, text) \
     VALUES ($1, clock_timestamp(), $2, $3)";
const PRUNE: &str = "DELETE FROM dns_zone_log WHERE zone_id = $1 AND id <= \
     (SELECT id FROM dns_zone_log WHERE zone_id = $1 ORDER BY id DESC OFFSET $2 LIMIT 1)";
const NEWEST: &str = "SELECT id, at, level, text FROM \
     (SELECT id, at, level, text FROM dns_zone_log WHERE zone_id = $1 ORDER BY id DESC LIMIT $2) \
     newest ORDER BY id";
const AFTER: &str = "SELECT id, at, level, text FROM dns_zone_log \
     WHERE zone_id = $1 AND id > $2 ORDER BY id LIMIT $3";

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(crate = "keel::sdk::serde")]
#[schemars(crate = "keel::sdk::schemars")]
pub(crate) struct Watched {
    id: Id,
}

pub(crate) fn log() -> FunctionBuilder<Watched, LogLine> {
    function::<Watched, LogLine>(LOG)
        .access(Level::User, READ)
        .feature(FEATURE)
        .owned::<zone::Entity>()
        .ui(ObjectScreen::new("dns.zone.log.title"))
}

pub(crate) async fn tail(call: Call<Watched>) -> Result<Lines> {
    let db = call
        .db()
        .connection()
        .cloned()
        .ok_or_else(|| KeelError::unavailable(UNREADABLE))?;
    let zone = call.params().id.to_string();
    let backlog = read(&db, NEWEST, [zone.as_str().into(), BACKLOG.into()]).await?;
    let cursor = backlog.last().map_or(0, |(id, _)| *id);
    let follow = Follow {
        db,
        zone,
        cursor,
        pending: backlog.into_iter().map(|(_, line)| line).collect(),
        failed: false,
    };
    Ok(Lines::new(stream::unfold(follow, Follow::next)))
}

pub(crate) async fn applied<C: ConnectionTrait>(db: &C, zone: &zone::Model) -> Result<()> {
    let text = format!("serial {} applied", zone.serial);
    record(db, &zone.id, INFO, &text).await
}

pub(crate) async fn failed<C: ConnectionTrait>(
    db: &C,
    zone: &zone::Model,
    detail: &str,
) -> Result<()> {
    let text = format!("serial {} failed: {detail}", zone.serial);
    record(db, &zone.id, ERROR, &text).await
}

async fn record<C: ConnectionTrait>(db: &C, zone: &str, level: &str, text: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        INSERT,
        [zone.into(), level.into(), text.into()],
    ))
    .await?;
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        PRUNE,
        [zone.into(), KEEP.into()],
    ))
    .await?;
    Ok(())
}

struct Follow {
    db: DatabaseConnection,
    zone: String,
    cursor: i64,
    pending: VecDeque<LogLine>,
    failed: bool,
}

impl Follow {
    async fn next(mut self) -> Option<(LogLine, Self)> {
        loop {
            if let Some(line) = self.pending.pop_front() {
                return Some((line, self));
            }
            if self.failed {
                return None;
            }
            keel::sdk::tokio::time::sleep(POLL).await;
            let values = [
                self.zone.as_str().into(),
                self.cursor.into(),
                BACKLOG.into(),
            ];
            match read(&self.db, AFTER, values).await {
                Ok(fresh) => {
                    if let Some((id, _)) = fresh.last() {
                        self.cursor = *id;
                    }
                    self.pending.extend(fresh.into_iter().map(|(_, line)| line));
                }
                Err(error) => {
                    self.failed = true;
                    self.pending.push_back(LogLine::new(
                        LogLevel::Error,
                        format!("the zone history could not be read: {error}"),
                    ));
                }
            }
        }
    }
}

async fn read<I>(db: &DatabaseConnection, sql: &str, values: I) -> Result<Vec<(i64, LogLine)>>
where
    I: IntoIterator<Item = keel::sdk::sea_orm::Value>,
{
    db.query_all(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await?
    .iter()
    .map(line_of)
    .collect()
}

fn line_of(row: &QueryResult) -> Result<(i64, LogLine)> {
    let id: i64 = row.try_get("", "id")?;
    let at: DateTime<Utc> = row.try_get("", "at")?;
    let level: String = row.try_get("", "level")?;
    let text: String = row.try_get("", "text")?;
    let level = match level.as_str() {
        ERROR => LogLevel::Error,
        "warn" => LogLevel::Warn,
        _ => LogLevel::Info,
    };
    Ok((id, LogLine::new(level, text).at(at)))
}
