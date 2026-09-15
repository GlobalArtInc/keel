use std::fmt;
use std::sync::OnceLock;

use keel_sdk::KeelError;
use keel_sdk::async_trait::async_trait;
use keel_sdk::system::{AdvisoryLock, LockGuard};
use sea_orm::DatabaseConnection;
use sea_orm::sqlx::pool::PoolConnection;
use sea_orm::sqlx::{self, Executor, Postgres};

use super::StorageError;

pub const MIGRATIONS: i64 = i64::from_be_bytes(*b"keelmigr");
pub const KERNEL_LEDGER: i64 = i64::from_be_bytes(*b"keelledg");
pub const SCHEMA: i64 = i64::from_be_bytes(*b"keelschm");
pub const INSTANCE: i64 = i64::from_be_bytes(*b"keelinst");
pub const TASKS: i64 = i64::from_be_bytes(*b"keeltask");
pub const CRON: i64 = i64::from_be_bytes(*b"keelcron");
pub const OUTBOX: i64 = i64::from_be_bytes(*b"keeloutb");

pub struct MigrationLock {
    connection: PoolConnection<Postgres>,
}

impl MigrationLock {
    pub(super) async fn take(db: &DatabaseConnection) -> Result<Self, StorageError> {
        let connection = pinned(db, MIGRATIONS).await.map_err(lock_failed)?;
        Ok(Self { connection })
    }

    pub async fn release(mut self) {
        let release = format!("SELECT pg_advisory_unlock({MIGRATIONS})");
        let _ = self.connection.execute(release.as_str()).await;
    }
}

async fn pinned(
    db: &DatabaseConnection,
    key: i64,
) -> Result<PoolConnection<Postgres>, sqlx::Error> {
    let mut connection = db.get_postgres_connection_pool().acquire().await?;
    connection.close_on_drop();
    let take = format!("SELECT pg_advisory_lock({key})");
    connection.execute(take.as_str()).await?;
    Ok(connection)
}

fn lock_failed(source: sqlx::Error) -> StorageError {
    StorageError::Lock {
        source: sea_orm::DbErr::Conn(sea_orm::RuntimeErr::SqlxError(source)),
    }
}

#[derive(Default)]
pub struct SessionLocks {
    db: OnceLock<DatabaseConnection>,
}

impl SessionLocks {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach(&self, db: DatabaseConnection) -> bool {
        self.db.set(db).is_ok()
    }

    pub fn is_attached(&self) -> bool {
        self.db.get().is_some()
    }
}

#[async_trait]
impl AdvisoryLock for SessionLocks {
    async fn take(&self, key: i64) -> Result<LockGuard, KeelError> {
        let db = match self.db.get() {
            Some(db) if !matches!(db, DatabaseConnection::Disconnected) => db,
            _ => {
                return Err(KeelError::unavailable("keel.error.lock_unavailable").param("key", key));
            }
        };
        let connection = pinned(db, key).await.map_err(|source| {
            KeelError::unavailable("keel.error.lock_failed")
                .param("key", key)
                .source(source)
        })?;
        Ok(LockGuard::new(connection))
    }
}

impl fmt::Debug for SessionLocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionLocks")
            .field("attached", &self.is_attached())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lock_key_is_distinct() {
        let keys = [MIGRATIONS, KERNEL_LEDGER, SCHEMA, INSTANCE, TASKS, CRON];
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[index + 1..].contains(key), "{key}");
        }
    }

    #[tokio::test]
    async fn a_take_before_the_pool_is_attached_is_unavailable() {
        let locks = SessionLocks::new();
        assert!(!locks.is_attached());
        let error = locks.take(TASKS).await.unwrap_err();
        assert_eq!(error.key(), "keel.error.lock_unavailable");
        assert!(locks.attach(DatabaseConnection::Disconnected));
        assert!(!locks.attach(DatabaseConnection::Disconnected));
        assert!(locks.is_attached());
        let error = locks.take(TASKS).await.unwrap_err();
        assert_eq!(error.key(), "keel.error.lock_unavailable");
        assert_eq!(format!("{locks:?}"), "SessionLocks { attached: true }");
    }
}
