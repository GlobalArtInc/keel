use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, DbErr, Statement, Value};

use super::composition::Graph;
use crate::migrations;
use crate::storage::{Storage, StorageError};

pub const ACTIVE: &str = "active";
pub const ORPHANED: &str = "orphaned";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migrated {
    pub kernel: Vec<String>,
    pub modules: Vec<(String, Vec<String>)>,
    pub orphaned: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error(transparent)]
    Kernel(#[from] StorageError),
    #[error("module {module}: {source}")]
    Module {
        module: String,
        #[source]
        source: StorageError,
    },
    #[error("could not record the modules in keel_module: {0}")]
    Record(#[source] DbErr),
}

impl Graph {
    pub async fn migrate(&self, storage: &Storage) -> Result<Migrated, MigrateError> {
        let lock = storage.lock_migrations().await?;
        let result = self.migrate_locked(storage).await;
        lock.release().await;
        let migrated = result?;
        tracing::info!(
            kernel = ?migrated.kernel,
            modules = ?migrated.modules,
            orphaned = ?migrated.orphaned,
            "migrations are up to date"
        );
        Ok(migrated)
    }

    async fn migrate_locked(&self, storage: &Storage) -> Result<Migrated, MigrateError> {
        let mut migrated = Migrated {
            kernel: storage.apply(&migrations::KERNEL).await?,
            ..Migrated::default()
        };
        for def in self.modules() {
            let applied = match def.migrations {
                Some(catalogue) => {
                    storage
                        .apply(catalogue)
                        .await
                        .map_err(|source| MigrateError::Module {
                            module: def.id.to_owned(),
                            source,
                        })?
                }
                None => Vec::new(),
            };
            record_active(storage.db(), def.id, def.version)
                .await
                .map_err(MigrateError::Record)?;
            migrated.modules.push((def.id.to_owned(), applied));
        }
        migrated.orphaned = record_orphaned(storage.db(), &self.ids())
            .await
            .map_err(MigrateError::Record)?;
        Ok(migrated)
    }
}

async fn record_active(db: &DatabaseConnection, id: &str, version: &str) -> Result<(), DbErr> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO keel_module (id, version, state, installed_at, updated_at) \
         VALUES ($1, $2, $3, now(), now()) \
         ON CONFLICT (id) DO UPDATE SET version = EXCLUDED.version, state = EXCLUDED.state, updated_at = now() \
         WHERE keel_module.version IS DISTINCT FROM EXCLUDED.version OR keel_module.state <> EXCLUDED.state",
        [Value::from(id), Value::from(version), Value::from(ACTIVE)],
    ))
    .await?;
    Ok(())
}

async fn record_orphaned(db: &DatabaseConnection, composed: &[&str]) -> Result<Vec<String>, DbErr> {
    let placeholders: Vec<String> = (0..composed.len())
        .map(|index| format!("${}", index + 2))
        .collect();
    let exclusion = if placeholders.is_empty() {
        String::new()
    } else {
        format!(" AND id NOT IN ({})", placeholders.join(", "))
    };
    let values =
        std::iter::once(Value::from(ORPHANED)).chain(composed.iter().map(|id| Value::from(*id)));
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "UPDATE keel_module SET state = $1, updated_at = now() \
                 WHERE state <> $1{exclusion} RETURNING id"
            ),
            values,
        ))
        .await?;
    rows.into_iter()
        .map(|row| row.try_get::<String>("", "id"))
        .collect()
}
