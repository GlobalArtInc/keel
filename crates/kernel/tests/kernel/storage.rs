use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use keel_kernel::Storage;
use keel_kernel::config::SCHEMA;
use keel_kernel::migrations::{LEDGER, TABLES};
use keel_kernel::storage::SessionLocks;
use keel_kernel::storage::lock::{CRON, TASKS};
use keel_sdk::system::AdvisoryLock;
use keel_testkit::TestDatabase;
use keel_types::Id;
use sea_orm::sqlx::Postgres;
use sea_orm::sqlx::pool::PoolConnection;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

async fn tables(storage: &Storage) -> BTreeSet<String> {
    storage
        .db()
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT table_name FROM information_schema.tables WHERE table_schema = $1",
            [SCHEMA.into()],
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "table_name").unwrap())
        .collect()
}

fn expected_tables() -> BTreeSet<String> {
    TABLES
        .iter()
        .map(|table| (*table).to_owned())
        .chain([format!("{LEDGER}_checksum")])
        .collect()
}

#[tokio::test]
async fn a_fresh_database_gets_the_eight_kernel_tables_and_the_ledger() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    assert!(tables(&storage).await.is_empty());

    let applied = storage.migrate().await.unwrap();
    assert_eq!(applied.len(), 3, "{applied:?}");
    assert_eq!(tables(&storage).await, expected_tables());
    assert_eq!(TABLES.len(), 8);

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_second_migration_run_changes_nothing_and_keeps_the_ledger_row_count() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let rows = test
        .count(&storage, "SELECT count(*) FROM keel_migration")
        .await;
    assert_eq!(rows, 3);

    let applied = storage.migrate().await.unwrap();
    assert!(applied.is_empty(), "{applied:?}");
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration")
            .await,
        rows
    );
    assert_eq!(tables(&storage).await, expected_tables());

    let status = keel_kernel::migrations::KERNEL
        .status(storage.db())
        .await
        .unwrap();
    assert!(status.unknown.is_empty());
    assert!(status.entries.iter().all(|entry| entry.applied));

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn the_root_account_is_the_only_account_with_kind_system_and_path_root() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();

    let rows = storage
        .db()
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id, kind, parent_id, path, state FROM keel_account",
        ))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let root = &rows[0];
    assert_eq!(
        root.try_get::<String>("", "id").unwrap(),
        Id::ROOT_ACCOUNT.to_string()
    );
    assert_eq!(root.try_get::<String>("", "kind").unwrap(), "system");
    assert_eq!(
        root.try_get::<Option<String>>("", "parent_id").unwrap(),
        None
    );
    assert_eq!(root.try_get::<String>("", "path").unwrap(), "/");
    assert_eq!(root.try_get::<String>("", "state").unwrap(), "active");

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn the_pool_works_inside_the_installation_schema() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    assert_eq!(storage.schema(), SCHEMA);
    let search_path = storage
        .db()
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SHOW search_path",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert_eq!(search_path, SCHEMA);

    storage.migrate().await.unwrap();
    let schema_of_ledger = storage
        .db()
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT table_schema FROM information_schema.tables WHERE table_name = 'keel_migration'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<String>(0)
        .unwrap();
    assert_eq!(schema_of_ledger, SCHEMA);

    let again = test.storage().await;
    assert_eq!(again.schema(), SCHEMA);
    again.close().await.unwrap();
    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_migrations_serialise_on_the_advisory_lock_and_leave_one_ledger() {
    let test = TestDatabase::create().await;
    let first = test.storage().await;
    let second = test.storage().await;

    let (left, right) = tokio::join!(first.migrate(), second.migrate());
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.len() + right.len(), 3, "{left:?} {right:?}");
    assert!(left.is_empty() || right.is_empty(), "{left:?} {right:?}");

    assert_eq!(
        test.count(&first, "SELECT count(*) FROM keel_migration")
            .await,
        3
    );
    assert_eq!(
        test.count(&first, "SELECT count(*) FROM keel_account")
            .await,
        1
    );
    assert_eq!(tables(&first).await, expected_tables());

    first.close().await.unwrap();
    second.close().await.unwrap();
    test.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_migration_waits_while_another_process_holds_the_lock() {
    let test = TestDatabase::create().await;
    let holder = test.storage().await;
    let waiter = test.storage().await;

    let lock = holder.lock_migrations().await.unwrap();
    let migrating = tokio::spawn(async move {
        let applied = waiter.migrate().await.unwrap();
        (waiter, applied)
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!migrating.is_finished());
    assert!(
        tables(&holder).await.is_empty(),
        "the waiter must not touch the schema before the lock is released"
    );

    lock.release().await;
    let (waiter, applied) = tokio::time::timeout(Duration::from_secs(30), migrating)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(applied.len(), 3);
    assert_eq!(tables(&holder).await, expected_tables());

    waiter.close().await.unwrap();
    holder.close().await.unwrap();
    test.finish().await;
}

async fn advisory_waiters(storage: &Storage) -> i64 {
    storage
        .db()
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND wait_event = 'advisory'",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get_by_index::<i64>(0)
        .unwrap()
}

async fn settle(storage: &Storage, waiters: i64) {
    for _ in 0..100 {
        if advisory_waiters(storage).await == waiters {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the advisory waiters did not settle at {waiters}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_taker_blocks_until_the_guard_drops_and_a_dropped_pending_take_closes_its_connection()
 {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    let locks = Arc::new(SessionLocks::new());
    assert!(locks.attach(storage.db().clone()));
    let pool = storage.db().get_postgres_connection_pool();

    let guard = locks.take(TASKS).await.unwrap();
    assert!(guard.held::<PoolConnection<Postgres>>().is_some());
    let cron = locks.take(CRON).await.unwrap();
    assert!(pool.size() >= 2, "{}", pool.size());

    let waiting_locks = locks.clone();
    let waiting = tokio::spawn(async move { waiting_locks.take(TASKS).await });
    settle(&storage, 1).await;
    assert!(!waiting.is_finished());

    drop(guard);
    let second = tokio::time::timeout(Duration::from_secs(30), waiting)
        .await
        .expect("the waiter takes over once the guard drops")
        .expect("the waiter did not panic")
        .expect("the lock is taken");
    settle(&storage, 0).await;

    let pending_locks = locks.clone();
    let pending = tokio::spawn(async move { pending_locks.take(TASKS).await });
    settle(&storage, 1).await;
    let before = pool.size();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    for _ in 0..100 {
        if pool.size() < before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pool.size() < before, "{} < {before}", pool.size());

    drop(second);
    settle(&storage, 0).await;
    let third = tokio::time::timeout(Duration::from_secs(30), locks.take(TASKS))
        .await
        .expect("nothing is left behind by the dropped take")
        .unwrap();
    drop(third);
    drop(cron);
    storage.close().await.unwrap();
    test.finish().await;
}
