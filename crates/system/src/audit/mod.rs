pub mod api;

mod entity;
mod functions;
mod sink;

use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::system::{AuditSink, ContextExt};

pub use functions::PERMISSION_READ as READ;
pub use sink::{BATCH_ROWS, BATCH_WAIT, CHANNEL_CAPACITY, STATUS_COMMITTED, STATUS_FAILED};

pub const FUNCTIONS: [&str; 2] = ["audit.list", "audit.get"];
pub const MENU_PATH: &str = "system/audit";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/audit").table("keel_migration_audit");

pub static MODULE: ModuleDef = ModuleDef {
    id: "audit",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let (sink, worker) = sink::channel();
    let recorder: Arc<dyn AuditSink> = sink.clone();
    let stopped = sink.clone();
    r.provide::<dyn AuditSink>(recorder)
        .provide::<api::Flush>(Arc::new(api::Flush::new(sink)))
        .function(functions::list())
        .function(functions::get())
        .permission(READ, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, "audit.list")
                .icon("scroll-text")
                .order(50),
        )
        .i18n("en", include_str!("../../i18n/audit/en.json"))
        .i18n("ru", include_str!("../../i18n/audit/ru.json"))
        .start(move |ctx| async move {
            let db = ctx.db();
            ctx.spawn(worker.run(db));
            Ok(())
        })
        .stop(move || async move { stopped.stop().await });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_functions_permission_menu_node_and_the_sink() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode, Level)> = registrations
            .functions
            .iter()
            .map(|def| (def.name(), def.kind(), def.mode(), def.access().level))
            .collect();
        assert_eq!(
            functions,
            [
                ("audit.list", Kind::List, Mode::Read, Level::User),
                ("audit.get", Kind::Get, Mode::Read, Level::User),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert_eq!(def.access().permission.as_deref(), Some(READ));
            assert_eq!(def.key(), Some("id"));
            assert_eq!(
                def.owned().map(|owned| owned.table.as_str()),
                Some("audit_call")
            );
        }
        let permissions: Vec<(&str, &[Level])> = registrations
            .permissions
            .iter()
            .map(|def| (def.key.as_str(), def.defaults.as_slice()))
            .collect();
        assert_eq!(permissions, [(READ, &[Level::User, Level::Reseller][..])]);
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), "audit.list");
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.provides.get::<dyn AuditSink>().is_some());
        assert!(registrations.provides.get::<api::Flush>().is_some());
        assert!(registrations.requires.is_empty());
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_some());
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_audit")
        );
    }
}
