mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use keel_kernel::Storage;
use keel_kernel::config::SCHEMA;
use keel_kernel::migrations::{LEDGER, TABLES};
use keel_types::Id;
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
    let test = common::create().await;
    let storage = test.storage().await;
    assert!(tables(&storage).await.is_empty());

    let applied = storage.migrate().await.unwrap();
    assert_eq!(applied.len(), 2, "{applied:?}");
    assert_eq!(tables(&storage).await, expected_tables());
    assert_eq!(TABLES.len(), 8);

    storage.close().await.unwrap();
    test.drop().await;
}

#[tokio::test]
async fn a_second_migration_run_changes_nothing_and_keeps_the_ledger_row_count() {
    let test = common::create().await;
    let storage = test.storage().await;
    storage.migrate().await.unwrap();
    let rows = test
        .count(&storage, "SELECT count(*) FROM keel_migration")
        .await;
    assert_eq!(rows, 2);

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
    test.drop().await;
}

#[tokio::test]
async fn the_root_account_is_the_only_account_with_kind_system_and_path_root() {
    let test = common::create().await;
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
    test.drop().await;
}

#[tokio::test]
async fn the_pool_works_inside_the_installation_schema() {
    let test = common::create().await;
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
    test.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_migrations_serialise_on_the_advisory_lock_and_leave_one_ledger() {
    let test = common::create().await;
    let first = test.storage().await;
    let second = test.storage().await;

    let (left, right) = tokio::join!(first.migrate(), second.migrate());
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left.len() + right.len(), 2, "{left:?} {right:?}");
    assert!(left.is_empty() || right.is_empty(), "{left:?} {right:?}");

    assert_eq!(
        test.count(&first, "SELECT count(*) FROM keel_migration")
            .await,
        2
    );
    assert_eq!(
        test.count(&first, "SELECT count(*) FROM keel_account")
            .await,
        1
    );
    assert_eq!(tables(&first).await, expected_tables());

    first.close().await.unwrap();
    second.close().await.unwrap();
    test.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_migration_waits_while_another_process_holds_the_lock() {
    let test = common::create().await;
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
    assert_eq!(applied.len(), 2);
    assert_eq!(tables(&holder).await, expected_tables());

    waiter.close().await.unwrap();
    holder.close().await.unwrap();
    test.drop().await;
}
