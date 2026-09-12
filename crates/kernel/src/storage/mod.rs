use std::time::Duration;

use keel_sdk::migrator::{self, Migrations};
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbErr, TransactionTrait,
};

use crate::config::Config;
use crate::migrations;

pub(crate) mod lock;

pub use lock::MigrationLock;

pub const POOL_SIZE: u32 = 20;
pub const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct Storage {
    db: DatabaseConnection,
    schema: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("could not connect to {database}: {source}")]
    Connect {
        database: String,
        #[source]
        source: DbErr,
    },
    #[error("could not prepare the schema \"{schema}\": {source}")]
    Schema {
        schema: String,
        #[source]
        source: DbErr,
    },
    #[error("could not take the migration lock: {source}")]
    Lock {
        #[source]
        source: DbErr,
    },
    #[error("migrations failed: {0}")]
    Migration(#[from] migrator::Error),
    #[error("the migration task ended early: {0}")]
    Interrupted(#[from] tokio::task::JoinError),
    #[error("the database refused a query: {0}")]
    Query(#[from] DbErr),
}

impl Storage {
    pub async fn connect(config: &Config) -> Result<Self, StorageError> {
        let schema = config.product.clone();
        let mut options = ConnectOptions::new(config.database.expose());
        options
            .max_connections(POOL_SIZE)
            .connect_timeout(TIMEOUT)
            .acquire_timeout(TIMEOUT)
            .set_schema_search_path(schema.clone())
            .sqlx_logging(false);
        let db = Database::connect(options)
            .await
            .map_err(|source| StorageError::Connect {
                database: config.database.to_string(),
                source,
            })?;
        create_schema(&db, &schema).await?;
        tracing::info!(database = %config.database, schema, "storage connected");
        Ok(Self { db, schema })
    }

    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub async fn lock_migrations(&self) -> Result<MigrationLock, StorageError> {
        MigrationLock::take(&self.db).await
    }

    pub async fn migrate(&self) -> Result<Vec<String>, StorageError> {
        let lock = self.lock_migrations().await?;
        let applied = self.apply(&migrations::KERNEL).await?;
        lock.release().await;
        tracing::info!(
            applied = ?applied,
            ledger = migrations::LEDGER,
            "kernel migrations are up to date"
        );
        Ok(applied)
    }

    pub async fn apply(&self, catalogue: &'static Migrations) -> Result<Vec<String>, StorageError> {
        let db = self.db.clone();
        let handle = tokio::runtime::Handle::current();
        let applied =
            tokio::task::spawn_blocking(move || handle.block_on(catalogue.apply(&db))).await??;
        Ok(applied)
    }

    pub async fn close(self) -> Result<(), StorageError> {
        self.db.close().await.map_err(StorageError::Query)
    }
}

async fn create_schema(db: &DatabaseConnection, schema: &str) -> Result<(), StorageError> {
    let failed = |source| StorageError::Schema {
        schema: schema.to_owned(),
        source,
    };
    let txn = db.begin().await.map_err(failed)?;
    txn.execute_unprepared(&format!("SELECT pg_advisory_xact_lock({})", lock::SCHEMA))
        .await
        .map_err(failed)?;
    txn.execute_unprepared(&format!("CREATE SCHEMA IF NOT EXISTS \"{schema}\""))
        .await
        .map_err(failed)?;
    txn.commit().await.map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DatabaseUrl;

    #[test]
    fn a_connect_error_names_the_database_without_its_password() {
        let url =
            DatabaseUrl::parse("postgres://keel_demo:s3cret@db.example.net/keel_demo").unwrap();
        let error = StorageError::Connect {
            database: url.to_string(),
            source: DbErr::Custom("refused".to_owned()),
        };
        let message = error.to_string();
        assert!(
            message.contains("keel_demo:***@db.example.net"),
            "{message}"
        );
        assert!(!message.contains("s3cret"), "{message}");
        assert!(message.contains("refused"), "{message}");
    }
}
