use keel_sdk::migrator::{self, Migrations};

use crate::storage::lock;

pub static KERNEL: Migrations = migrator::embed!("$CARGO_MANIFEST_DIR/migrations")
    .table("keel_migration")
    .lock_key(lock::KERNEL_LEDGER);

pub const LEDGER: &str = "keel_migration";

pub const TABLES: [&str; 8] = [
    "keel_account",
    "keel_principal",
    "keel_migration",
    "keel_module",
    "keel_kv",
    "keel_outbox",
    "keel_idempotency",
    "keel_limit",
];

#[cfg(test)]
mod tests {
    use keel_types::Id;

    use super::*;

    #[test]
    fn the_kernel_catalogue_carries_its_files_under_the_kernel_ledger() {
        assert_eq!(KERNEL.ledger(), LEDGER);
        let names: Vec<String> = KERNEL
            .all()
            .unwrap()
            .into_iter()
            .map(|migration| migration.name)
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].contains("CreateKernelTables"), "{names:?}");
        assert!(names[1].contains("InsertRootAccount"), "{names:?}");
    }

    #[test]
    fn every_migration_has_a_down_file() {
        for migration in KERNEL.all().unwrap() {
            assert!(migration.down.is_some(), "{}", migration.name);
        }
    }

    #[test]
    fn the_migration_creates_every_kernel_table_and_the_root_account() {
        let sql: String = KERNEL
            .all()
            .unwrap()
            .into_iter()
            .map(|migration| migration.up)
            .collect();
        for table in TABLES.iter().filter(|table| **table != LEDGER) {
            assert!(sql.contains(&format!("CREATE TABLE {table} (")), "{table}");
        }
        assert!(sql.contains(&format!("'{}', 'system', NULL, '/'", Id::ROOT_ACCOUNT)));
    }

    #[test]
    fn the_root_account_id_is_the_nil_identifier() {
        let sql = KERNEL
            .all()
            .unwrap()
            .into_iter()
            .find(|migration| migration.name.contains("InsertRootAccount"))
            .unwrap()
            .up;
        assert_eq!(Id::ROOT_ACCOUNT, Id::from(ulid::Ulid::nil()));
        assert!(sql.contains("'00000000000000000000000000'"), "{sql}");
        assert!(sql.contains(&format!("'{}'", Id::ROOT_ACCOUNT)), "{sql}");
    }
}
