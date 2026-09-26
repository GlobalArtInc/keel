use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use futures::StreamExt;
use keel_sdk::sea_orm::DatabaseConnection;
use keel_sdk::sea_orm::sqlx::postgres::PgConnection;
use keel_sdk::sea_orm::sqlx::{self, Row};
use keel_sdk::system::BackupTable;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::BackupError;
use crate::instance::{KV_INSTANCE_KEY, KV_SECRET_KEY};
use crate::migrations::LEDGER;
use crate::secret::{Key, Sealed};

const TABLE_EXTENSION: &str = "copy";
const CHUNK: usize = 64 * 1024;
const LEDGER_PREFIX: &str = "keel_migration";
const CHECKSUM_SUFFIX: &str = "_checksum";

const SCHEMA: &str = "SELECT current_schema()::text";
const TABLES: &str = "SELECT table_name::text FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' ORDER BY table_name";
const COLUMNS: &str = "SELECT column_name::text FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = $1 AND is_generated = 'NEVER' ORDER BY ordinal_position";
const SEQUENCED: &str = "SELECT column_name::text FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = $1 AND (is_identity = 'YES' OR column_default LIKE 'nextval(%') ORDER BY ordinal_position";
const FOREIGN_KEYS: &str = "SELECT child.relname::text, parent.relname::text FROM pg_constraint c JOIN pg_class child ON child.oid = c.conrelid JOIN pg_class parent ON parent.oid = c.confrelid JOIN pg_namespace n ON n.oid = child.relnamespace WHERE c.contype = 'f' AND n.nspname = current_schema()";
const PRODUCTS: &str = "SELECT value::text FROM keel_kv WHERE key = 'products'";
const SECRET_KEY: &str = "SELECT value::text FROM keel_kv WHERE key = $1";
const DROP_SECRET_KEY: &str = "DELETE FROM keel_kv WHERE key = $1";
const BEGIN_SNAPSHOT: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY";
const BEGIN: &str = "BEGIN";
const COMMIT: &str = "COMMIT";
const ROLLBACK: &str = "ROLLBACK";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    pub(super) applied: u32,
    pub(super) head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dumped {
    pub(super) schema: String,
    pub(super) tables: Vec<BackupTable>,
    pub(super) ledgers: BTreeMap<String, Ledger>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKey {
    Kept,
    Dropped,
    Absent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub loaded: Vec<(String, u64)>,
    pub skipped: Vec<String>,
    pub truncated: Vec<String>,
    pub products_before: Vec<String>,
    pub products_after: Vec<String>,
    pub data_key: DataKey,
}

fn is_ledger(table: &str) -> bool {
    table == LEDGER
        || (table.starts_with(&format!("{LEDGER_PREFIX}_")) && !table.ends_with(CHECKSUM_SUFFIX))
}

fn is_ledger_owned(table: &str) -> bool {
    table.starts_with(LEDGER_PREFIX)
}

fn file_of(dir: &Path, table: &str) -> PathBuf {
    dir.join(format!("{table}.{TABLE_EXTENSION}"))
}

async fn acquire(
    db: &DatabaseConnection,
) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, BackupError> {
    let mut connection = db
        .get_postgres_connection_pool()
        .acquire()
        .await
        .map_err(sql("acquire a connection"))?;
    connection.close_on_drop();
    Ok(connection)
}

pub async fn write(db: &DatabaseConnection, dir: &Path) -> Result<Dumped, BackupError> {
    let mut held = acquire(db).await?;
    let conn: &mut PgConnection = &mut held;
    tokio::fs::create_dir_all(dir).await.map_err(create(dir))?;
    sqlx::query(BEGIN_SNAPSHOT)
        .execute(&mut *conn)
        .await
        .map_err(sql("begin the snapshot"))?;
    let dumped = copy_all(conn, dir).await;
    let ended = sqlx::query(COMMIT).execute(&mut *conn).await;
    let dumped = dumped?;
    ended.map_err(sql("end the snapshot"))?;
    Ok(dumped)
}

async fn copy_all(conn: &mut PgConnection, dir: &Path) -> Result<Dumped, BackupError> {
    let schema: String = sqlx::query(SCHEMA)
        .fetch_one(&mut *conn)
        .await
        .map_err(sql("read the schema"))?
        .try_get(0)
        .map_err(sql("read the schema"))?;
    let names = table_names(conn).await?;
    let mut tables = Vec::with_capacity(names.len());
    let mut ledgers = BTreeMap::new();
    for name in names {
        let columns = columns_of(conn, &name).await?;
        copy_out(conn, dir, &schema, &name, &columns).await?;
        if is_ledger(&name) {
            ledgers.insert(name.clone(), ledger_of(conn, &schema, &name).await?);
        }
        tables.push(BackupTable { name, columns });
    }
    Ok(Dumped {
        schema,
        tables,
        ledgers,
    })
}

async fn table_names(conn: &mut PgConnection) -> Result<Vec<String>, BackupError> {
    sqlx::query(TABLES)
        .fetch_all(&mut *conn)
        .await
        .map_err(sql("list the tables"))?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<Result<_, _>>()
        .map_err(sql("list the tables"))
}

async fn columns_of(conn: &mut PgConnection, table: &str) -> Result<Vec<String>, BackupError> {
    sqlx::query(COLUMNS)
        .bind(table)
        .fetch_all(&mut *conn)
        .await
        .map_err(sql(&format!("list the columns of {table}")))?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<Result<_, _>>()
        .map_err(sql(&format!("list the columns of {table}")))
}

async fn ledger_of(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<Ledger, BackupError> {
    let statement = format!(
        "SELECT count(*)::bigint, (SELECT name FROM {0}.{1} ORDER BY timestamp DESC, id DESC LIMIT 1) FROM {0}.{1}",
        quote(schema),
        quote(table)
    );
    let row = sqlx::query(&statement)
        .fetch_one(&mut *conn)
        .await
        .map_err(sql(&format!("read the ledger {table}")))?;
    let applied: i64 = row
        .try_get(0)
        .map_err(sql(&format!("read the ledger {table}")))?;
    let head: Option<String> = row
        .try_get(1)
        .map_err(sql(&format!("read the ledger {table}")))?;
    Ok(Ledger {
        applied: u32::try_from(applied).unwrap_or(u32::MAX),
        head,
    })
}

async fn copy_out(
    conn: &mut PgConnection,
    dir: &Path,
    schema: &str,
    table: &str,
    columns: &[String],
) -> Result<(), BackupError> {
    let path = file_of(dir, table);
    let mut file = File::create(&path).await.map_err(create(&path))?;
    let statement = format!(
        "COPY {}.{} ({}) TO STDOUT",
        quote(schema),
        quote(table),
        column_list(columns)
    );
    let mut stream = conn
        .copy_out_raw(&statement)
        .await
        .map_err(sql(&format!("copy {table} out")))?;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(sql(&format!("copy {table} out")))?;
        file.write_all(&chunk).await.map_err(write_failed(&path))?;
    }
    file.flush().await.map_err(write_failed(&path))?;
    Ok(())
}

pub async fn load(
    db: &DatabaseConnection,
    dir: &Path,
    tables: &[BackupTable],
    file_key: Option<&Key>,
) -> Result<Loaded, BackupError> {
    let mut held = acquire(db).await?;
    let conn: &mut PgConnection = &mut held;
    sqlx::query(BEGIN)
        .execute(&mut *conn)
        .await
        .map_err(sql("begin the restore"))?;
    let loaded = copy_all_in(conn, dir, tables, file_key).await;
    match loaded {
        Ok(loaded) => {
            sqlx::query(COMMIT)
                .execute(&mut *conn)
                .await
                .map_err(sql("commit the restore"))?;
            Ok(loaded)
        }
        Err(error) => {
            let _ = sqlx::query(ROLLBACK).execute(&mut *conn).await;
            Err(error)
        }
    }
}

async fn copy_all_in(
    conn: &mut PgConnection,
    dir: &Path,
    tables: &[BackupTable],
    file_key: Option<&Key>,
) -> Result<Loaded, BackupError> {
    let schema: String = sqlx::query(SCHEMA)
        .fetch_one(&mut *conn)
        .await
        .map_err(sql("read the schema"))?
        .try_get(0)
        .map_err(sql("read the schema"))?;
    let products_before = products_of(conn).await?;
    let existing: BTreeSet<String> = table_names(conn).await?.into_iter().collect();
    let truncated: Vec<String> = existing
        .iter()
        .filter(|table| !is_ledger_owned(table))
        .cloned()
        .collect();
    if !truncated.is_empty() {
        let statement = format!(
            "TRUNCATE {}",
            truncated
                .iter()
                .map(|table| format!("{}.{}", quote(&schema), quote(table)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        sqlx::query(&statement)
            .execute(&mut *conn)
            .await
            .map_err(sql("truncate the tables"))?;
    }
    let edges = foreign_keys(conn).await?;
    let mut skipped = Vec::new();
    let mut pending: Vec<&BackupTable> = Vec::new();
    for table in tables {
        if is_ledger_owned(&table.name) {
            continue;
        }
        if existing.contains(&table.name) {
            pending.push(table);
        } else {
            skipped.push(table.name.clone());
        }
    }
    let mut loaded = Vec::with_capacity(pending.len());
    for table in ordered(pending, &edges) {
        let rows = copy_in(conn, dir, &schema, table).await?;
        realign_sequences(conn, &schema, &table.name).await?;
        loaded.push((table.name.clone(), rows));
    }
    let products_after = products_of(conn).await?;
    let data_key = reconcile_data_key(conn, file_key).await?;
    Ok(Loaded {
        loaded,
        skipped,
        truncated,
        products_before,
        products_after,
        data_key,
    })
}

async fn realign_sequences(
    conn: &mut PgConnection,
    schema: &str,
    table: &str,
) -> Result<(), BackupError> {
    let step = format!("realign the sequences of {table}");
    let columns: Vec<String> = sqlx::query(SEQUENCED)
        .bind(table)
        .fetch_all(&mut *conn)
        .await
        .map_err(sql(&step))?
        .iter()
        .map(|row| row.try_get::<String, _>(0))
        .collect::<Result<_, _>>()
        .map_err(sql(&step))?;
    for column in columns {
        let qualified = format!("{}.{}", quote(schema), quote(table));
        let statement = format!(
            "SELECT setval(pg_get_serial_sequence($1, $2), COALESCE((SELECT max({}) FROM {qualified}), 0) + 1, false)",
            quote(&column)
        );
        sqlx::query(&statement)
            .bind(&qualified)
            .bind(&column)
            .execute(&mut *conn)
            .await
            .map_err(sql(&step))?;
    }
    Ok(())
}

async fn products_of(conn: &mut PgConnection) -> Result<Vec<String>, BackupError> {
    let row = sqlx::query(PRODUCTS)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sql("read the enabled products"))?;
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let text: String = row.try_get(0).map_err(sql("read the enabled products"))?;
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    Ok(value
        .as_object()
        .map(|set| set.keys().cloned().collect())
        .unwrap_or_default())
}

async fn reconcile_data_key(
    conn: &mut PgConnection,
    file_key: Option<&Key>,
) -> Result<DataKey, BackupError> {
    let row = sqlx::query(SECRET_KEY)
        .bind(KV_SECRET_KEY)
        .fetch_optional(&mut *conn)
        .await
        .map_err(sql("read the wrapped data key"))?;
    let Some(row) = row else {
        return Ok(DataKey::Absent);
    };
    let text: String = row.try_get(0).map_err(sql("read the wrapped data key"))?;
    let opens = file_key.is_some_and(|key| {
        serde_json::from_str::<Sealed>(&text).is_ok_and(|sealed| key.open(&sealed).is_ok())
    });
    if opens {
        return Ok(DataKey::Kept);
    }
    for key in [KV_SECRET_KEY, KV_INSTANCE_KEY] {
        sqlx::query(DROP_SECRET_KEY)
            .bind(key)
            .execute(&mut *conn)
            .await
            .map_err(sql("drop the wrapped data key"))?;
    }
    Ok(DataKey::Dropped)
}

async fn foreign_keys(conn: &mut PgConnection) -> Result<Vec<(String, String)>, BackupError> {
    sqlx::query(FOREIGN_KEYS)
        .fetch_all(&mut *conn)
        .await
        .map_err(sql("list the foreign keys"))?
        .iter()
        .map(|row| Ok((row.try_get::<String, _>(0)?, row.try_get::<String, _>(1)?)))
        .collect::<Result<_, sqlx::Error>>()
        .map_err(sql("list the foreign keys"))
}

fn ordered<'a>(tables: Vec<&'a BackupTable>, edges: &[(String, String)]) -> Vec<&'a BackupTable> {
    let names: BTreeSet<&str> = tables.iter().map(|table| table.name.as_str()).collect();
    let mut parents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (child, parent) in edges {
        if child != parent && names.contains(child.as_str()) && names.contains(parent.as_str()) {
            parents
                .entry(child.as_str())
                .or_default()
                .insert(parent.as_str());
        }
    }
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut ordered = Vec::with_capacity(tables.len());
    let mut remaining = tables;
    while !remaining.is_empty() {
        let (ready, blocked): (Vec<_>, Vec<_>) = remaining.into_iter().partition(|table| {
            parents
                .get(table.name.as_str())
                .is_none_or(|needed| needed.iter().all(|parent| placed.contains(parent)))
        });
        if ready.is_empty() {
            ordered.extend(blocked);
            break;
        }
        for table in ready {
            placed.insert(table.name.as_str());
            ordered.push(table);
        }
        remaining = blocked;
    }
    ordered
}

async fn copy_in(
    conn: &mut PgConnection,
    dir: &Path,
    schema: &str,
    table: &BackupTable,
) -> Result<u64, BackupError> {
    let path = file_of(dir, &table.name);
    let mut file = File::open(&path).await.map_err(read_failed(&path))?;
    let statement = format!(
        "COPY {}.{} ({}) FROM STDIN",
        quote(schema),
        quote(&table.name),
        column_list(&table.columns)
    );
    let step = format!("copy {} in", table.name);
    let mut copy = conn.copy_in_raw(&statement).await.map_err(sql(&step))?;
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = match file.read(&mut buffer).await {
            Ok(read) => read,
            Err(source) => {
                let _ = copy.abort("the dump file could not be read").await;
                return Err(BackupError::Read {
                    path: path.clone(),
                    source,
                });
            }
        };
        if read == 0 {
            break;
        }
        if let Err(source) = copy.send(&buffer[..read]).await {
            return Err(sql(&step)(source));
        }
    }
    copy.finish().await.map_err(sql(&step))
}

fn column_list(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| quote(column))
        .collect::<Vec<_>>()
        .join(", ")
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn sql(step: &str) -> impl FnOnce(sqlx::Error) -> BackupError {
    let step = step.to_owned();
    move |source| BackupError::Database { step, source }
}

fn create(path: &Path) -> impl FnOnce(std::io::Error) -> BackupError {
    let path = path.to_owned();
    move |source| BackupError::Create { path, source }
}

fn write_failed(path: &Path) -> impl FnOnce(std::io::Error) -> BackupError {
    let path = path.to_owned();
    move |source| BackupError::Write { path, source }
}

fn read_failed(path: &Path) -> impl FnOnce(std::io::Error) -> BackupError {
    let path = path.to_owned();
    move |source| BackupError::Read { path, source }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(name: &str) -> BackupTable {
        BackupTable {
            name: name.to_owned(),
            columns: vec!["id".to_owned()],
        }
    }

    #[test]
    fn ledgers_are_told_apart_from_data_tables() {
        assert!(is_ledger("keel_migration"));
        assert!(is_ledger("keel_migration_auth"));
        assert!(!is_ledger("keel_migration_checksum"));
        assert!(!is_ledger("keel_migration_auth_checksum"));
        assert!(!is_ledger("keel_kv"));
        assert!(is_ledger_owned("keel_migration_auth_checksum"));
        assert!(!is_ledger_owned("task_job"));
        assert_eq!(
            file_of(Path::new("/tmp/db"), "task_job"),
            PathBuf::from("/tmp/db/task_job.copy")
        );
        assert_eq!(quote("odd\"name"), "\"odd\"\"name\"");
        assert_eq!(
            column_list(&["id".to_owned(), "title".to_owned()]),
            "\"id\", \"title\""
        );
    }

    #[test]
    fn tables_are_loaded_parents_first_and_a_cycle_is_left_in_place() {
        let session = table("auth_session");
        let principal = table("keel_principal");
        let account = table("keel_account");
        let kv = table("keel_kv");
        let edges = vec![
            ("auth_session".to_owned(), "keel_principal".to_owned()),
            ("auth_session".to_owned(), "keel_account".to_owned()),
            ("keel_principal".to_owned(), "keel_account".to_owned()),
            ("keel_account".to_owned(), "keel_account".to_owned()),
            ("other".to_owned(), "keel_kv".to_owned()),
        ];
        let ordered: Vec<&str> = ordered(vec![&session, &principal, &account, &kv], &edges)
            .into_iter()
            .map(|table| table.name.as_str())
            .collect();
        assert_eq!(
            ordered,
            ["keel_account", "keel_kv", "keel_principal", "auth_session"]
        );
        let a = table("a");
        let b = table("b");
        let cycle = vec![
            ("a".to_owned(), "b".to_owned()),
            ("b".to_owned(), "a".to_owned()),
        ];
        let stuck: Vec<&str> = super::ordered(vec![&a, &b], &cycle)
            .into_iter()
            .map(|table| table.name.as_str())
            .collect();
        assert_eq!(stuck, ["a", "b"]);
    }
}
