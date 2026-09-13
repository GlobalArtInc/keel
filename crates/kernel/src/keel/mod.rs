mod accounts;
mod acl;
mod entity;
mod principals;
mod products;
mod source;

use keel_sdk::prelude::*;
use keel_sdk::{ModuleDef, Registrar};

pub use accounts::{RESELLER_KIND, SYSTEM_KIND};
pub use principals::{DEFAULT_LANG, DEFAULT_ROLE};
pub(crate) use source::{StoredAccounts, StoredPrincipals};

pub static MODULE: ModuleDef = ModuleDef {
    id: "keel",
    version: env!("CARGO_PKG_VERSION"),
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

pub const FUNCTIONS: [&str; 12] = [
    "keel.account.list",
    "keel.account.get",
    "keel.account.create",
    "keel.account.update",
    "keel.account.options",
    "keel.principal.list",
    "keel.principal.get",
    "keel.principal.create",
    "keel.principal.update",
    "keel.product.list",
    "keel.product.enable",
    "keel.product.disable",
];

#[derive(Deserialize, Serialize, JsonSchema)]
struct Pick {
    pub id: Id,
}

#[derive(Deserialize, Serialize, JsonSchema)]
struct Saved {
    pub id: Id,
}

fn init(r: &mut Registrar) -> Result<()> {
    r.function(accounts::list())
        .function(accounts::get())
        .function(accounts::create())
        .function(accounts::update())
        .function(accounts::options())
        .function(principals::list())
        .function(principals::get())
        .function(principals::create())
        .function(principals::update())
        .function(products::list())
        .function(products::enable())
        .function(products::disable())
        .permission("keel.account.read", &[Level::User, Level::Reseller])
        .permission("keel.account.manage", &[Level::Reseller])
        .permission("keel.principal.read", &[Level::User, Level::Reseller])
        .permission("keel.principal.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("system/accounts", "keel.account.list")
                .icon("users")
                .order(10)
                .label("keel.account.list.title"),
        )
        .menu(
            MenuNode::new("system/principals", "keel.principal.list")
                .icon("user")
                .order(20)
                .label("keel.principal.list.title"),
        )
        .menu(
            MenuNode::new("system/products", "keel.product.list")
                .icon("package")
                .order(30)
                .label("keel.product.list.title"),
        )
        .i18n("en", include_str!("../../i18n/en.json"))
        .i18n("ru", include_str!("../../i18n/ru.json"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_module_registers_its_twelve_functions_under_the_keel_prefix() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let names: Vec<&str> = registrar.function_names().into_iter().collect();
        let mut expected = FUNCTIONS.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        assert_eq!(registrations.permissions.len(), 4);
        assert_eq!(registrations.menu.len(), 3);
        assert!(
            registrations
                .menu
                .iter()
                .all(|node| node.path().starts_with("system/"))
        );
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(MODULE.sdk, keel_sdk::VERSION);
        assert!(MODULE.migrations.is_none());
    }
}
