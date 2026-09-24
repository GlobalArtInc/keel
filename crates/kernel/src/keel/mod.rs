mod accounts;
mod acl;
mod entity;
mod health;
mod lifecycle;
mod principals;
mod products;
mod restart;
mod setup;
mod source;

use keel_sdk::prelude::*;
use keel_sdk::{ModuleDef, Registrar};

pub use accounts::{RESELLER_KIND, SYSTEM_KIND};
use health::DOCTOR_RUN;
pub use health::LOGS_TAIL;
pub use principals::{DEFAULT_LANG, DEFAULT_ROLE};
pub use restart::RESTART;
use setup::SETUP_ADMIN;
pub use setup::{Handle as SetupHandle, KEY_PREFIX as SETUP_KEY_PREFIX, PATH as SETUP_PATH};
pub(crate) use source::{StoredAccounts, StoredPrincipals};

pub static MODULE: ModuleDef = ModuleDef {
    id: "keel",
    version: env!("CARGO_PKG_VERSION"),
    sdk: keel_sdk::VERSION,
    requires: &[],
    migrations: None,
    init,
};

pub const FUNCTIONS: [&str; 20] = [
    "keel.export",
    DOCTOR_RUN,
    LOGS_TAIL,
    RESTART,
    SETUP_ADMIN,
    "keel.account.list",
    "keel.account.get",
    "keel.account.create",
    "keel.account.update",
    "keel.account.options",
    accounts::SUSPEND,
    accounts::RESUME,
    accounts::DELETE,
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
        .function(accounts::suspend())
        .function(accounts::resume())
        .function(accounts::delete())
        .function(principals::list())
        .function(principals::get())
        .function(principals::create())
        .function(principals::update())
        .function(products::list())
        .function(products::enable())
        .function(products::disable())
        .function(restart::restart())
        .function(setup::admin())
        .function(crate::export::function())
        .function(health::doctor_run())
        .function(health::logs_tail())
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
        .menu(
            MenuNode::new(health::MENU_PATH, DOCTOR_RUN)
                .icon(health::MENU_ICON)
                .order(90)
                .label("keel.doctor.run.title"),
        )
        .i18n("en", include_str!("../../i18n/en.json"))
        .i18n("ru", include_str!("../../i18n/ru.json"))
        .start(|ctx| async move {
            if setup::issue_when_unclaimed(&ctx).await? {
                tracing::info!("no administrator exists; a setup link is issued for this start");
            }
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::serde_json::{self, Value};

    use super::*;

    #[test]
    fn the_kernel_module_registers_its_twenty_functions_under_the_keel_prefix() {
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
        assert_eq!(registrations.menu.len(), 4);
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

    fn leaves_under(dictionary: &str, path: &[&str]) -> Vec<(String, Value)> {
        let mut messages: Value = serde_json::from_str(dictionary).expect("the dictionary is JSON");
        for segment in path {
            messages = messages[*segment].take();
        }
        let Value::Object(map) = messages else {
            panic!("{} is not an object", path.join("."));
        };
        crate::loader::leaves(&map)
    }

    #[test]
    fn the_dictionaries_carry_the_phoenix_labels_and_the_shell_strings_in_both_languages() {
        let en = include_str!("../../i18n/en.json");
        let ru = include_str!("../../i18n/ru.json");
        for path in [&["phx"][..], &["keel", "shell"][..]] {
            let english = leaves_under(en, path);
            let russian = leaves_under(ru, path);
            assert!(english.len() >= 40, "{}: {}", path.join("."), english.len());
            let keys = |leaves: &[(String, Value)]| -> Vec<String> {
                leaves.iter().map(|(key, _)| key.clone()).collect()
            };
            assert_eq!(keys(&english), keys(&russian), "{}", path.join("."));
            for ((key, english), (_, russian)) in english.iter().zip(&russian) {
                assert!(english.is_string(), "{key} in en");
                assert!(russian.is_string(), "{key} in ru");
                assert_ne!(english, russian, "{key} is not translated");
            }
        }
        let phx = leaves_under(en, &["phx"]);
        for label in [
            "close",
            "cancel",
            "search",
            "nothingFound",
            "loadFailed",
            "retry",
            "perPage",
            "selectAll",
            "save",
        ] {
            assert!(
                phx.iter().any(|(key, _)| key == label),
                "phx.{label} is missing"
            );
        }
        let shell = leaves_under(en, &["keel", "shell"]);
        for key in [
            "login.title",
            "acting_as.banner",
            "user.logout",
            "security.title",
        ] {
            assert!(
                shell.iter().any(|(leaf, _)| leaf == key),
                "keel.shell.{key}"
            );
        }
    }
}
