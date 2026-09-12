use std::collections::BTreeMap;

use keel_kernel::config::SCHEMA;
use keel_kernel::loader::{ACTIVE, MigrateError, Migrated, ORPHANED};
use keel_kernel::{BootError, Composition, Kernel, Storage};
use keel_sdk::prelude::*;
use keel_sdk::{Registrar, migrator};
use keel_testkit::TestDatabase;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

static BETA_MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/modules/beta/migrations")
        .table("keel_migration_beta");

static ALPHA_MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/modules/alpha/migrations")
        .table("keel_migration_alpha");

static BROKEN_MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/tests/modules/broken/migrations")
        .table("keel_migration_broken");

fn init(_: &mut Registrar) -> Result<()> {
    Ok(())
}

static BETA: ModuleDef = ModuleDef {
    id: "beta",
    version: "1.0.0",
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: Some(&BETA_MIGRATIONS),
    init,
};

static ALPHA: ModuleDef = ModuleDef {
    id: "alpha",
    version: "1.2.0",
    sdk: keel_sdk::VERSION,
    requires: &["beta"],
    migrations: Some(&ALPHA_MIGRATIONS),
    init,
};

static GAMMA: ModuleDef = ModuleDef {
    id: "gamma",
    version: "0.3.0",
    sdk: keel_sdk::VERSION,
    requires: &["alpha"],
    migrations: None,
    init,
};

static BROKEN: ModuleDef = ModuleDef {
    id: "broken",
    version: "0.0.1",
    sdk: keel_sdk::VERSION,
    requires: &["beta"],
    migrations: Some(&BROKEN_MIGRATIONS),
    init,
};

async fn migrate(storage: &Storage, modules: &[&'static ModuleDef]) -> Migrated {
    let mut composition = Composition::new();
    for module in modules {
        composition = composition.module(module);
    }
    composition
        .resolve()
        .expect("the composition resolves")
        .migrate(storage)
        .await
        .expect("migrations run")
}

async fn tables(storage: &Storage) -> Vec<String> {
    storage
        .db()
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT table_name FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name",
            [SCHEMA.into()],
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.try_get::<String>("", "table_name").unwrap())
        .collect()
}

async fn modules(storage: &Storage) -> BTreeMap<String, (String, String)> {
    storage
        .db()
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT id, version, state FROM keel_module",
        ))
        .await
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.try_get::<String>("", "id").unwrap(),
                (
                    row.try_get::<String>("", "version").unwrap(),
                    row.try_get::<String>("", "state").unwrap(),
                ),
            )
        })
        .collect()
}

fn row(version: &str, state: &str) -> (String, String) {
    (version.to_owned(), state.to_owned())
}

#[tokio::test]
async fn migrations_run_kernel_first_then_modules_in_graph_order_under_their_own_ledgers() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;

    let migrated = migrate(&storage, &[&GAMMA, &ALPHA, &BETA]).await;
    assert_eq!(migrated.kernel.len(), 2, "{migrated:?}");
    assert_eq!(
        migrated.modules,
        [
            ("keel".to_owned(), Vec::new()),
            (
                "beta".to_owned(),
                vec!["CreateBetaThing1789300000000".to_owned()]
            ),
            (
                "alpha".to_owned(),
                vec!["CreateAlphaThing1789300000001".to_owned()]
            ),
            ("gamma".to_owned(), Vec::new()),
        ]
    );
    assert!(migrated.orphaned.is_empty());

    let present = tables(&storage).await;
    for table in [
        "alpha_thing",
        "beta_thing",
        "keel_migration",
        "keel_migration_alpha",
        "keel_migration_alpha_checksum",
        "keel_migration_beta",
        "keel_migration_beta_checksum",
    ] {
        assert!(
            present.iter().any(|name| name == table),
            "{table} in {present:?}"
        );
    }
    assert!(!present.iter().any(|name| name == "keel_migration_gamma"));
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration")
            .await,
        2
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration_beta")
            .await,
        1
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration_alpha")
            .await,
        1
    );
    assert_eq!(
        modules(&storage).await,
        BTreeMap::from([
            ("alpha".to_owned(), row("1.2.0", ACTIVE)),
            ("beta".to_owned(), row("1.0.0", ACTIVE)),
            ("gamma".to_owned(), row("0.3.0", ACTIVE)),
            ("keel".to_owned(), row(KERNEL_VERSION, ACTIVE)),
        ])
    );

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_second_start_applies_nothing_and_keeps_every_ledger_as_it_was() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    migrate(&storage, &[&ALPHA, &BETA]).await;
    let before = modules(&storage).await;

    let again = migrate(&storage, &[&ALPHA, &BETA]).await;
    assert!(again.kernel.is_empty(), "{again:?}");
    assert_eq!(
        again.modules,
        [
            ("keel".to_owned(), Vec::new()),
            ("beta".to_owned(), Vec::new()),
            ("alpha".to_owned(), Vec::new())
        ]
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration")
            .await,
        2
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration_beta")
            .await,
        1
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration_alpha")
            .await,
        1
    );
    assert_eq!(modules(&storage).await, before);
    let untouched = test
        .count(
            &storage,
            "SELECT count(*) FROM keel_module WHERE installed_at = updated_at",
        )
        .await;
    assert_eq!(untouched, 3);

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn removing_a_module_leaves_its_tables_and_flips_its_row_to_orphaned_on_the_next_start() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;
    migrate(&storage, &[&ALPHA, &BETA]).await;

    let without_alpha = migrate(&storage, &[&BETA]).await;
    assert_eq!(without_alpha.orphaned, ["alpha"]);
    assert_eq!(
        modules(&storage).await,
        BTreeMap::from([
            ("alpha".to_owned(), row("1.2.0", ORPHANED)),
            ("beta".to_owned(), row("1.0.0", ACTIVE)),
            ("keel".to_owned(), row(KERNEL_VERSION, ACTIVE)),
        ])
    );
    let present = tables(&storage).await;
    assert!(
        present.iter().any(|name| name == "alpha_thing"),
        "{present:?}"
    );
    assert!(
        present.iter().any(|name| name == "keel_migration_alpha"),
        "{present:?}"
    );
    assert_eq!(
        test.count(&storage, "SELECT count(*) FROM keel_migration_alpha")
            .await,
        1
    );

    let with_alpha = migrate(&storage, &[&BETA, &ALPHA]).await;
    assert!(with_alpha.orphaned.is_empty());
    assert_eq!(with_alpha.modules[2], ("alpha".to_owned(), Vec::new()));
    assert_eq!(modules(&storage).await["alpha"], row("1.2.0", ACTIVE));

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn a_failing_module_migration_names_the_module_and_records_nothing_for_it() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;

    let error = Composition::new()
        .module(&BROKEN)
        .module(&BETA)
        .resolve()
        .unwrap()
        .migrate(&storage)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, MigrateError::Module { module, .. } if module == "broken"),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("module broken: migrations failed:"),
        "{error}"
    );
    assert_eq!(
        modules(&storage).await,
        BTreeMap::from([
            ("beta".to_owned(), row("1.0.0", ACTIVE)),
            ("keel".to_owned(), row(KERNEL_VERSION, ACTIVE)),
        ])
    );

    storage.close().await.unwrap();
    test.finish().await;
}

#[tokio::test]
async fn the_kernel_boots_a_composition_against_the_database_and_stops_in_reverse() {
    let test = TestDatabase::create().await;
    let storage = test.storage().await;

    let composition = Composition::new()
        .module(&GAMMA)
        .module(&BETA)
        .module(&ALPHA);
    let kernel = Kernel::boot(composition, &storage).await.unwrap();
    assert_eq!(kernel.graph().ids(), ["keel", "beta", "alpha", "gamma"]);
    assert_eq!(
        kernel.running().collect::<Vec<_>>(),
        ["keel", "beta", "alpha", "gamma"]
    );
    let ids: Vec<&str> = kernel
        .registry()
        .modules()
        .iter()
        .map(|module| module.id.as_str())
        .collect();
    assert_eq!(ids, ["keel", "beta", "alpha", "gamma"]);
    assert_eq!(
        kernel.registry().functions().count(),
        keel_kernel::keel::FUNCTIONS.len()
    );
    kernel.stop().await.unwrap();

    let error = Kernel::boot(Composition::new().module(&ALPHA), &storage)
        .await
        .unwrap_err();
    assert!(matches!(error, BootError::Composition(_)), "{error}");
    assert_eq!(
        error.to_string(),
        "the module composition is invalid:\n  - module alpha requires beta, which is not part of the composition"
    );

    storage.close().await.unwrap();
    test.finish().await;
}
