use std::path::{Path, PathBuf};

use futures::StreamExt;
use keel_sdk::chrono::{DateTime, Utc};
use keel_sdk::sea_orm::DatabaseConnection;
use keel_sdk::sea_orm::sqlx::postgres::PgConnection;
use keel_sdk::sea_orm::sqlx::{self, Row};
use keel_sdk::serde::Serialize;
use keel_sdk::serde_json;
use keel_sdk::{Db, KeelError, Result};
use tokio::io::AsyncWriteExt;

pub const DIRECTORY: &str = "backups";
pub const TABLE_EXTENSION: &str = "copy";
pub const MANIFEST: &str = "manifest.json";
const ERROR_DUMP: &str = "update.error.dump";

const TABLES: &str = "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' ORDER BY table_name";
const SCHEMA: &str = "SELECT current_schema()::text";
const BEGIN: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY";
const COMMIT: &str = "COMMIT";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
pub(super) struct Dumped {
    pub(super) dir: PathBuf,
    pub(super) schema: String,
    pub(super) tables: Vec<String>,
    pub(super) at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(crate = "keel_sdk::serde", rename_all = "camelCase")]
struct Manifest<'a> {
    keel: &'a str,
    reason: &'a str,
    schema: &'a str,
    format: &'static str,
    tables: &'a [String],
    at: DateTime<Utc>,
}

pub(super) fn dir_of(data_dir: &Path, running: &str, at: DateTime<Utc>) -> PathBuf {
    data_dir.join(DIRECTORY).join(format!(
        "pre-update-{running}-{}",
        at.format("%Y%m%dT%H%M%SZ")
    ))
}

pub(super) async fn write(
    db: &Db,
    dir: &Path,
    running: &str,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<Dumped> {
    let connection = db
        .connection()
        .ok_or_else(|| KeelError::internal(ERROR_DUMP).param("reason", "no pool connection"))?;
    if !matches!(
        connection,
        DatabaseConnection::SqlxPostgresPoolConnection(_)
    ) {
        return Err(KeelError::internal(ERROR_DUMP).param("reason", "not a postgres pool"));
    }
    let mut held = connection
        .get_postgres_connection_pool()
        .acquire()
        .await
        .map_err(sql("acquire"))?;
    let conn: &mut PgConnection = &mut held;
    tokio::fs::create_dir_all(dir).await.map_err(io(dir))?;
    sqlx::query(BEGIN)
        .execute(&mut *conn)
        .await
        .map_err(sql("begin"))?;
    let dumped = copy_all(conn, dir, running, reason, at).await;
    let ended = sqlx::query(COMMIT).execute(&mut *conn).await;
    let dumped = dumped?;
    ended.map_err(sql("commit"))?;
    Ok(dumped)
}

async fn copy_all(
    conn: &mut PgConnection,
    dir: &Path,
    running: &str,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<Dumped> {
    let schema: String = sqlx::query(SCHEMA)
        .fetch_one(&mut *conn)
        .await
        .map_err(sql("schema"))?
        .try_get(0)
        .map_err(sql("schema"))?;
    let tables: Vec<String> = sqlx::query(TABLES)
        .fetch_all(&mut *conn)
        .await
        .map_err(sql("tables"))?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<std::result::Result<_, _>>()
        .map_err(sql("tables"))?;
    for table in &tables {
        copy_table(conn, dir, &schema, table).await?;
    }
    let manifest = dir.join(MANIFEST);
    let text = serde_json::to_vec_pretty(&Manifest {
        keel: running,
        reason,
        schema: &schema,
        format: "copy-text",
        tables: &tables,
        at,
    })?;
    tokio::fs::write(&manifest, text)
        .await
        .map_err(io(&manifest))?;
    Ok(Dumped {
        dir: dir.to_owned(),
        schema,
        tables,
        at,
    })
}

async fn copy_table(conn: &mut PgConnection, dir: &Path, schema: &str, table: &str) -> Result<()> {
    let path = dir.join(format!("{table}.{TABLE_EXTENSION}"));
    let mut file = tokio::fs::File::create(&path).await.map_err(io(&path))?;
    let statement = format!(
        "COPY {}.{} TO STDOUT",
        quote_identifier(schema),
        quote_identifier(table)
    );
    let mut stream = conn.copy_out_raw(&statement).await.map_err(sql(table))?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sql(table))?;
        file.write_all(&chunk).await.map_err(io(&path))?;
    }
    file.flush().await.map_err(io(&path))?;
    Ok(())
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn sql(step: &str) -> impl FnOnce(sqlx::Error) -> KeelError {
    let step = step.to_owned();
    move |error| {
        KeelError::internal(ERROR_DUMP)
            .param("step", step)
            .param("reason", error.to_string())
            .source(error)
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> KeelError {
    let path = path.display().to_string();
    move |error| {
        KeelError::internal(ERROR_DUMP)
            .param("path", path)
            .param("reason", error.to_string())
            .source(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dump_directory_names_the_running_version_and_the_instant() {
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        assert_eq!(
            dir_of(Path::new("/var/lib/keel"), "0.1.0", at),
            PathBuf::from("/var/lib/keel/backups/pre-update-0.1.0-20260921T101530Z")
        );
        assert_eq!(quote_identifier("task_job"), "\"task_job\"");
        assert_eq!(quote_identifier("odd\"name"), "\"odd\"\"name\"");
    }
}
