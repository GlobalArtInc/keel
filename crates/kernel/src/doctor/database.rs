use std::fmt;

use keel_sdk::migrator::Migrations;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, Value};
use serde::Serialize;

use super::{ACCOUNTS, Check, DATABASE, Findings, MIGRATIONS, ORPHANS, PRODUCTS};
use crate::config::SCHEMA;
use crate::loader::ORPHANED;
use crate::products;

const MINIMUM_VERSION: i32 = 150_000;
const MINIMUM_NAME: &str = "15";
const SERVER: &str = "SELECT current_setting('server_version_num')::int AS num, current_setting('server_version')::text AS version, current_user::text AS role, inet_server_addr() IS NULL AS via_socket, current_schema()::text AS schema, has_schema_privilege(current_schema(), 'CREATE') AS can_create";
const TABLE_EXISTS: &str = "SELECT 1 AS present FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = $1";
const ORPHANED_MODULES: &str = "SELECT id FROM keel_module WHERE state = $1 ORDER BY id";
const DELETING_ACCOUNTS: &str = "SELECT id FROM keel_account WHERE state = 'deleting' ORDER BY id";
const DELETING_STATE: &str = "deleting";

pub enum Database<'a> {
    Connected(&'a DatabaseConnection),
    Unreachable(String),
}

impl fmt::Debug for Database<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Database::Connected(_) => f.write_str("Database::Connected"),
            Database::Unreachable(reason) => write!(f, "Database::Unreachable({reason:?})"),
        }
    }
}

#[derive(Clone)]
pub struct Ledger {
    pub(crate) module: String,
    pub(crate) table: String,
    pub(crate) catalogue: &'static Migrations,
}

impl fmt::Debug for Ledger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ledger")
            .field("module", &self.module)
            .field("table", &self.table)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerStatus {
    pub module: String,
    pub table: String,
    pub present: bool,
    pub applied: Vec<String>,
    pub pending: Vec<String>,
    pub unknown: Vec<String>,
}

pub(crate) fn statement(sql: &str, values: impl IntoIterator<Item = Value>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

fn quoted(table: &str) -> String {
    format!("\"{}\"", table.replace('"', "\"\""))
}

pub(crate) async fn table_exists(db: &DatabaseConnection, table: &str) -> Result<bool, DbErr> {
    Ok(db
        .query_one(statement(TABLE_EXISTS, [Value::from(table)]))
        .await?
        .is_some())
}

async fn column<T>(db: &DatabaseConnection, sql: &str, values: Vec<Value>) -> Result<Vec<T>, DbErr>
where
    T: sea_orm::TryGetable,
{
    db.query_all(statement(sql, values))
        .await?
        .into_iter()
        .map(|row| row.try_get_by_index::<T>(0))
        .collect()
}

pub async fn ledger_status(
    db: &DatabaseConnection,
    ledgers: &[Ledger],
) -> Result<Vec<LedgerStatus>, DbErr> {
    let mut statuses = Vec::with_capacity(ledgers.len());
    for ledger in ledgers {
        let carried: Vec<String> = ledger
            .catalogue
            .all()
            .map_err(|error| DbErr::Custom(format!("{}: {error}", ledger.module)))?
            .into_iter()
            .map(|migration| migration.name)
            .collect();
        let present = table_exists(db, &ledger.table).await?;
        let applied: Vec<String> = if present {
            column(
                db,
                &format!(
                    "SELECT name::text FROM {} ORDER BY timestamp, id",
                    quoted(&ledger.table)
                ),
                Vec::new(),
            )
            .await?
        } else {
            Vec::new()
        };
        statuses.push(LedgerStatus {
            module: ledger.module.clone(),
            table: ledger.table.clone(),
            present,
            pending: carried
                .iter()
                .filter(|name| !applied.contains(name))
                .cloned()
                .collect(),
            unknown: applied
                .iter()
                .filter(|name| !carried.contains(name))
                .cloned()
                .collect(),
            applied,
        });
    }
    Ok(statuses)
}

fn unreachable(id: &'static str, reason: &str) -> Check {
    Check::fail(id, format!("the database is unreachable: {reason}"))
}

fn refused(id: &'static str, error: &DbErr) -> Check {
    Check::fail(id, format!("the database refused the query: {error}"))
}

pub(super) async fn server(database: &Database<'_>) -> Check {
    let db = match database {
        Database::Connected(db) => *db,
        Database::Unreachable(reason) => return unreachable(DATABASE, reason),
    };
    let row = match db.query_one(statement(SERVER, [])).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Check::fail(DATABASE, "the server answered no row to the version query");
        }
        Err(error) => return refused(DATABASE, &error),
    };
    let read =
        |name: &str| -> Result<Option<String>, DbErr> { row.try_get::<Option<String>>("", name) };
    let outcome = (|| -> Result<Check, DbErr> {
        let mut findings = Findings::new(DATABASE);
        let num: i32 = row.try_get("", "num")?;
        let version = read("version")?.unwrap_or_default();
        let role = read("role")?.unwrap_or_default();
        let via_socket: bool = row
            .try_get::<Option<bool>>("", "via_socket")?
            .unwrap_or(false);
        let schema = read("schema")?;
        let can_create: Option<bool> = row.try_get("", "can_create")?;
        if num < MINIMUM_VERSION {
            findings.fail(format!(
                "PostgreSQL {version} is below the minimum {MINIMUM_NAME}"
            ));
        }
        match schema.as_deref() {
            Some(name) if name == SCHEMA => {}
            Some(name) => findings.fail(format!(
                "the schema {SCHEMA} does not exist; the connection landed in {name}"
            )),
            None => findings.fail(format!("the schema {SCHEMA} does not exist")),
        }
        if schema.is_some() && can_create != Some(true) {
            findings.fail(format!(
                "the role {role} may not create objects in the schema {SCHEMA}; migrations would fail"
            ));
        }
        let transport = if via_socket { "the unix socket" } else { "TCP" };
        Ok(findings.finish(format!(
            "PostgreSQL {version} over {transport} as {role}; the schema {SCHEMA} exists and the role may create in it"
        )))
    })();
    outcome.unwrap_or_else(|error| refused(DATABASE, &error))
}

pub(super) async fn migrations(database: &Database<'_>, ledgers: &[Ledger]) -> Check {
    let db = match database {
        Database::Connected(db) => *db,
        Database::Unreachable(reason) => return unreachable(MIGRATIONS, reason),
    };
    let statuses = match ledger_status(db, ledgers).await {
        Ok(statuses) => statuses,
        Err(error) => return refused(MIGRATIONS, &error),
    };
    let mut findings = Findings::new(MIGRATIONS);
    let mut applied = 0usize;
    for status in &statuses {
        applied += status.applied.len();
        if !status.present {
            findings.fail(format!(
                "the ledger {} of {} does not exist; the schema is not migrated",
                status.table, status.module
            ));
            continue;
        }
        if !status.pending.is_empty() {
            findings.fail(format!(
                "{} has {} pending migration(s) the binary carries: {}",
                status.module,
                status.pending.len(),
                status.pending.join(", ")
            ));
        }
        if !status.unknown.is_empty() {
            findings.fail(format!(
                "{} recorded {} migration(s) this binary does not carry: {}",
                status.module,
                status.unknown.len(),
                status.unknown.join(", ")
            ));
        }
    }
    findings.finish(format!(
        "{} ledger(s) match the binary, {applied} migration(s) applied",
        statuses.len()
    ))
}

pub(super) async fn products(database: &Database<'_>, carried: &[String]) -> Check {
    let db = match database {
        Database::Connected(db) => *db,
        Database::Unreachable(reason) => return unreachable(PRODUCTS, reason),
    };
    let enabled = match products::enabled(db).await {
        Ok(enabled) => enabled,
        Err(error) => {
            return Check::fail(PRODUCTS, format!("the enabled set cannot be read: {error}"));
        }
    };
    let mut findings = Findings::new(PRODUCTS);
    let mut names = Vec::new();
    for id in enabled.keys() {
        if carried.iter().any(|known| known == id) {
            names.push(id.clone());
        } else {
            findings.fail(format!(
                "{id} is enabled but not carried by this binary, which carries {}",
                if carried.is_empty() {
                    "no product bundle".to_owned()
                } else {
                    carried.join(", ")
                }
            ));
        }
    }
    let healthy = if names.is_empty() {
        "no product is enabled".to_owned()
    } else {
        format!("{} enabled and carried by the binary", names.join(", "))
    };
    findings.finish(healthy)
}

pub(super) async fn orphans(database: &Database<'_>) -> Check {
    let db = match database {
        Database::Connected(db) => *db,
        Database::Unreachable(reason) => return unreachable(ORPHANS, reason),
    };
    match column::<String>(db, ORPHANED_MODULES, vec![Value::from(ORPHANED)]).await {
        Ok(ids) if ids.is_empty() => Check::ok(ORPHANS, "no module is orphaned"),
        Ok(ids) => Check::warn(
            ORPHANS,
            format!(
                "{} orphaned module(s) whose tables remain after a disable: {}",
                ids.len(),
                ids.join(", ")
            ),
        ),
        Err(error) => refused(ORPHANS, &error),
    }
}

pub(super) async fn accounts(database: &Database<'_>) -> Check {
    let db = match database {
        Database::Connected(db) => *db,
        Database::Unreachable(reason) => return unreachable(ACCOUNTS, reason),
    };
    match column::<String>(db, DELETING_ACCOUNTS, Vec::new()).await {
        Ok(ids) if ids.is_empty() => {
            Check::ok(ACCOUNTS, format!("no account is left in {DELETING_STATE}"))
        }
        Ok(ids) => Check::warn(
            ACCOUNTS,
            format!(
                "{} account(s) left in {DELETING_STATE}; the delete job did not finish: {}",
                ids.len(),
                ids.join(", ")
            ),
        ),
        Err(error) => refused(ACCOUNTS, &error),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Status;
    use super::*;

    #[test]
    fn a_ledger_status_serialises_its_names_and_tables_are_quoted_for_sql() {
        let current = LedgerStatus {
            module: "keel".into(),
            table: "keel_migration".into(),
            present: true,
            applied: vec!["a".into()],
            pending: Vec::new(),
            unknown: Vec::new(),
        };
        let json = serde_json::to_value(&current).unwrap();
        assert_eq!(json["table"], "keel_migration");
        assert_eq!(json["applied"], serde_json::json!(["a"]));
        assert_eq!(
            format!(
                "{:?}",
                Ledger {
                    module: "keel".into(),
                    table: "keel_migration".into(),
                    catalogue: &crate::migrations::KERNEL,
                }
            ),
            "Ledger { module: \"keel\", table: \"keel_migration\", .. }"
        );
        assert_eq!(quoted("keel_migration"), "\"keel_migration\"");
        assert_eq!(quoted("odd\"name"), "\"odd\"\"name\"");
    }

    #[tokio::test]
    async fn an_unreachable_database_fails_every_database_check_with_the_reason() {
        let database = Database::Unreachable("connection refused".into());
        assert_eq!(
            format!("{database:?}"),
            "Database::Unreachable(\"connection refused\")"
        );
        for check in [
            server(&database).await,
            migrations(&database, &[]).await,
            products(&database, &[]).await,
            orphans(&database).await,
            accounts(&database).await,
        ] {
            assert_eq!(check.status, Status::Fail, "{}", check.id);
            assert_eq!(
                check.message,
                "the database is unreachable: connection refused"
            );
        }
    }
}
