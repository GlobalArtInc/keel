mod brand;
mod entity;
mod functions;
mod pref;
mod store;

use std::sync::Arc;

use keel_sdk::prelude::*;
use keel_sdk::system::{AccountSource, LanguageSource, ManifestSource, PreferenceSource};

use brand::Brands;
use pref::Prefs;
use store::Store;

pub use functions::{PERMISSION_MANAGE as MANAGE, PERMISSION_READ as READ, SettingRow, scope_of};
pub use pref::{LANG as PREF_LANG, THEME as PREF_THEME, TZ as PREF_TZ};

pub const FUNCTIONS: [&str; 9] = [
    "settings.list",
    "settings.get",
    "settings.set",
    "settings.pref.get",
    "settings.pref.set",
    "settings.brand.list",
    "settings.brand.get",
    "settings.brand.set",
    "settings.brand.delete",
];
pub const MENU_PATH: &str = "system/settings";
pub const BRAND_MENU_PATH: &str = "system/brand";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/settings").table("keel_migration_settings");

pub static MODULE: ModuleDef = ModuleDef {
    id: "settings",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let store = Arc::new(Store::default());
    let prefs = Arc::new(Prefs::default());
    r.provide::<dyn SettingsSource>(store.clone())
        .provide::<dyn PreferenceSource>(prefs.clone())
        .provide::<dyn ManifestSource>(Arc::new(Brands))
        .require::<dyn LanguageSource>()
        .function(functions::list(store.clone()))
        .function(functions::get(store.clone()))
        .function(functions::set(store.clone()))
        .function(pref::get())
        .function(pref::set(prefs.clone()))
        .function(brand::functions::list())
        .function(brand::functions::get())
        .function(brand::functions::set())
        .function(brand::functions::delete())
        .permission(READ, &[Level::Reseller])
        .permission(MANAGE, &[Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, "settings.list")
                .icon("sliders-horizontal")
                .order(40),
        )
        .menu(
            MenuNode::new(BRAND_MENU_PATH, brand::functions::LIST)
                .icon("palette")
                .order(45),
        )
        .i18n("en", include_str!("../../i18n/settings/en.json"))
        .i18n("ru", include_str!("../../i18n/settings/ru.json"))
        .start(move |ctx| async move {
            let accounts = ctx.require::<dyn AccountSource>()?;
            prefs.attach(ctx.require::<dyn LanguageSource>()?);
            store.attach(ctx.contributions::<SettingDef>(), accounts)
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn the_module_registers_its_functions_permissions_menu_node_and_the_source() {
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
                ("settings.list", Kind::List, Mode::Read, Level::Reseller),
                ("settings.get", Kind::Get, Mode::Read, Level::Reseller),
                ("settings.set", Kind::Form, Mode::Write, Level::Reseller),
                ("settings.pref.get", Kind::Get, Mode::Read, Level::User),
                ("settings.pref.set", Kind::Form, Mode::Write, Level::User),
                ("settings.brand.list", Kind::List, Mode::Read, Level::Admin),
                ("settings.brand.get", Kind::Get, Mode::Read, Level::Admin),
                ("settings.brand.set", Kind::Form, Mode::Write, Level::Admin),
                (
                    "settings.brand.delete",
                    Kind::Action,
                    Mode::Write,
                    Level::Admin
                ),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
        }
        let permissions: Vec<(&str, &[Level])> = registrations
            .permissions
            .iter()
            .map(|def| (def.key.as_str(), def.defaults.as_slice()))
            .collect();
        assert_eq!(
            permissions,
            [
                (READ, &[Level::Reseller][..]),
                (MANAGE, &[Level::Reseller][..])
            ]
        );
        assert_eq!(registrations.menu.len(), 2);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), "settings.list");
        assert_eq!(registrations.menu[1].path(), BRAND_MENU_PATH);
        assert_eq!(registrations.menu[1].func(), "settings.brand.list");
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.provides.get::<dyn SettingsSource>().is_some());
        assert!(
            registrations
                .provides
                .get::<dyn PreferenceSource>()
                .is_some()
        );
        assert!(registrations.provides.get::<dyn ManifestSource>().is_some());
        assert_eq!(
            registrations.requires,
            [keel_sdk::runtime::ServiceKey::of::<dyn LanguageSource>()]
        );
        assert!(registrations.start.is_some());
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_settings")
        );
    }

    #[test]
    fn an_admin_writes_the_root_scope_and_a_reseller_its_own_account() {
        let reseller: Id = "01J8A0000000000000000000R1".parse().expect("an id");
        let principal = |level: Level| Principal {
            id: Id::generate(),
            login: "p".into(),
            level,
            account_id: reseller,
            role: "owner".into(),
            actor: None,
            scope: keel_sdk::types::Scope::Full,
            via: keel_sdk::types::Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: BTreeMap::new(),
        };
        assert_eq!(scope_of(&principal(Level::Reseller)), reseller);
        assert_eq!(scope_of(&principal(Level::Admin)), Id::ROOT_ACCOUNT);
        assert_eq!(scope_of(&principal(Level::Owner)), Id::ROOT_ACCOUNT);
    }
}
