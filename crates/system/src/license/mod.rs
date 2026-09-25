mod functions;
mod grant;
mod lease;

use keel_sdk::prelude::*;
use keel_sdk::system::{Features, Installation, LicenseKey};

use grant::Grant;

pub use lease::{ERROR_INSTANCE, ERROR_LEASE, ERROR_SIGNATURE, KV_KEY};

pub const GET: &str = "license.get";
pub const INSTALL: &str = "license.install";
const MENU_PATH: &str = "system/license";
const MENU_ICON: &str = "key-round";

pub static MODULE: ModuleDef = ModuleDef {
    id: "license",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let grant = Arc::new(Grant::default());
    let started = grant.clone();
    r.provide::<dyn Features>(grant.clone())
        .require::<dyn LicenseKey>()
        .require::<Installation>()
        .function(functions::get(grant.clone()))
        .function(functions::install(grant))
        .menu(MenuNode::new(MENU_PATH, GET).icon(MENU_ICON).order(47))
        .i18n("en", include_str!("../../i18n/license/en.json"))
        .i18n("ru", include_str!("../../i18n/license/ru.json"))
        .start(move |ctx| async move { started.start(&ctx).await });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_screen_the_install_form_and_the_feature_set() {
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
                (GET, Kind::Get, Mode::Read, Level::Admin),
                (INSTALL, Kind::Form, Mode::Write, Level::Admin),
            ]
        );
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert!(def.owned().is_none());
            assert_eq!(def.access().permission, None);
            assert_eq!(def.access().feature, None);
        }
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), GET);
        assert!(MODULE.migrations.is_none());
    }
}
