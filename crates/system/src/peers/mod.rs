mod entity;
mod functions;

use keel_sdk::InstanceInfoSource;
use keel_sdk::prelude::*;

pub use functions::{ERROR_STORED, GET, LIST, STATUS_ACTIVE};

pub const FEATURE: &str = "peers";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/peers").table("keel_migration_peer");

pub static MODULE: ModuleDef = ModuleDef {
    id: "peer",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.contribute(InstanceInfoSource::new().feature(FEATURE))
        .function(functions::list())
        .function(functions::get())
        .i18n("en", include_str!("../../i18n/peers/en.json"))
        .i18n("ru", include_str!("../../i18n/peers/ru.json"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_two_readers_and_contributes_its_feature() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode, Level, bool)> = registrations
            .functions
            .iter()
            .map(|def| {
                (
                    def.name(),
                    def.kind(),
                    def.mode(),
                    def.access().level,
                    def.owned().is_some(),
                )
            })
            .collect();
        assert_eq!(
            functions,
            [
                (LIST, Kind::List, Mode::Read, Level::Admin, false),
                (GET, Kind::Get, Mode::Read, Level::Admin, false),
            ]
        );
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
        }
        let sources: Vec<&InstanceInfoSource> = registrations
            .contributions
            .of::<InstanceInfoSource>()
            .collect();
        assert_eq!(sources, [&InstanceInfoSource::new().feature(FEATURE)]);
        assert!(registrations.permissions.is_empty());
        assert!(registrations.menu.is_empty());
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(MODULE.id, "peer");
        assert!(MODULE.requires.is_empty());
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_peer")
        );
    }
}
