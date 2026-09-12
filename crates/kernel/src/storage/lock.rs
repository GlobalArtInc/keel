use sea_orm::DatabaseConnection;
use sea_orm::sqlx::pool::PoolConnection;
use sea_orm::sqlx::{self, Executor, Postgres};

use super::StorageError;

pub const MIGRATIONS: i64 = i64::from_be_bytes(*b"keelmigr");
pub const KERNEL_LEDGER: i64 = i64::from_be_bytes(*b"keelledg");
pub const SCHEMA: i64 = i64::from_be_bytes(*b"keelschm");
pub const INSTANCE: i64 = i64::from_be_bytes(*b"keelinst");

pub struct MigrationLock {
    connection: PoolConnection<Postgres>,
}

impl MigrationLock {
    pub(super) async fn take(db: &DatabaseConnection) -> Result<Self, StorageError> {
        let mut connection = db
            .get_postgres_connection_pool()
            .acquire()
            .await
            .map_err(lock_failed)?;
        connection.close_on_drop();
        let take = format!("SELECT pg_advisory_lock({MIGRATIONS})");
        connection
            .execute(take.as_str())
            .await
            .map_err(lock_failed)?;
        Ok(Self { connection })
    }

    pub async fn release(mut self) {
        let release = format!("SELECT pg_advisory_unlock({MIGRATIONS})");
        let _ = self.connection.execute(release.as_str()).await;
    }
}

fn lock_failed(source: sqlx::Error) -> StorageError {
    StorageError::Lock {
        source: sea_orm::DbErr::Conn(sea_orm::RuntimeErr::SqlxError(source)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lock_key_is_distinct() {
        let keys = [MIGRATIONS, KERNEL_LEDGER, SCHEMA, INSTANCE];
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[index + 1..].contains(key), "{key}");
        }
    }
}
