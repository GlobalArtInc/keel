mod accounts;
mod acl;
mod entity;
mod health;
mod idempotency;
mod lifecycle;
mod limits;
mod principals;
mod products;
mod restart;
mod setup;
mod source;
mod stats;
mod widgets;

use keel_sdk::prelude::*;
use keel_sdk::{MenuSection, ModuleDef, Registrar};

pub use accounts::{RESELLER_KIND, SYSTEM_KIND};
use health::DOCTOR_RUN;
pub use health::LOGS_TAIL;
pub use idempotency::PRUNE as IDEMPOTENCY_PRUNE;
pub(crate) use limits::StoredLimits;
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

pub const FUNCTIONS: [&str; 25] = [
    "keel.export",
    DOCTOR_RUN,
    LOGS_TAIL,
    health::LOGS_STREAM,
    RESTART,
    SETUP_ADMIN,
    idempotency::PRUNE,
    "keel.account.list",
    "keel.account.get",
    "keel.account.create",
    "keel.account.update",
    "keel.account.options",
    accounts::UPSERT,
    accounts::SUSPEND,
    accounts::RESUME,
    accounts::DELETE,
    "keel.principal.list",
    "keel.principal.get",
    "keel.principal.create",
    "keel.principal.update",
    principals::UPSERT,
    "keel.product.list",
    "keel.product.enable",
    "keel.product.disable",
    widgets::ACCOUNTS_WIDGET,
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
        .function(accounts::upsert())
        .function(accounts::suspend())
        .function(accounts::resume())
        .function(accounts::delete())
        .function(principals::list())
        .function(principals::get())
        .function(principals::create())
        .function(principals::update())
        .function(principals::upsert())
        .function(products::list())
        .function(products::enable())
        .function(products::disable())
        .function(restart::restart())
        .function(idempotency::prune())
        .function(setup::admin())
        .function(crate::export::function())
        .function(health::doctor_run())
        .function(health::logs_tail())
        .stream(health::logs_stream(), health::follow_logs)
        .function(widgets::accounts())
        .contribute(stats::accounts())
        .contribute(stats::principals())
        .contribute(widgets::widget())
        .contribute(idempotency::hours())
        .permission("keel.account.read", &[Level::User, Level::Reseller])
        .permission("keel.account.manage", &[Level::Reseller])
        .permission("keel.principal.read", &[Level::User, Level::Reseller])
        .permission("keel.principal.manage", &[Level::User, Level::Reseller])
        .section(MenuSection::new("access/users", "keel.menu.users", "users").order(10))
        .section(MenuSection::new("monitor/tasks", "keel.menu.tasks", "list-checks").order(20))
        .section(MenuSection::new("monitor/logs", "keel.menu.logs", "history").order(30))
        .section(
            MenuSection::new(
                "system/settings",
                "keel.menu.settings",
                "sliders-horizontal",
            )
            .order(10),
        )
        .section(MenuSection::new("system/install", "keel.menu.install", "package").order(30))
        .menu(
            MenuNode::new("access/users/principals", "keel.principal.list")
                .icon("user")
                .order(10)
                .label("keel.menu.principals"),
        )
        .menu(
            MenuNode::new("access/users/accounts", "keel.account.list")
                .icon("users")
                .order(20)
                .level(Level::Reseller)
                .label("keel.account.list.title"),
        )
        .menu(
            MenuNode::new("system/install/products", "keel.product.list")
                .icon("package")
                .order(10)
                .label("keel.product.list.title"),
        )
        .menu(
            MenuNode::new(health::MENU_PATH, DOCTOR_RUN)
                .icon(health::MENU_ICON)
                .order(40)
                .label("keel.doctor.run.title"),
        )
        .menu(
            MenuNode::new(health::LOGS_MENU_PATH, health::LOGS_STREAM)
                .icon(health::LOGS_MENU_ICON)
                .order(20)
                .label("keel.menu.server_log"),
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
    fn the_kernel_module_registers_its_twenty_five_functions_the_accounts_widget_and_the_idempotency_setting_under_the_keel_prefix()
     {
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
        assert_eq!(registrations.menu.len(), 5);
        let sections: Vec<(&str, i32)> = registrations
            .sections
            .iter()
            .map(|section| (section.path(), section.order_value()))
            .collect();
        assert_eq!(
            sections,
            [
                ("access/users", 10),
                ("monitor/tasks", 20),
                ("monitor/logs", 30),
                ("system/settings", 10),
                ("system/install", 30)
            ]
        );
        let widgets: Vec<(WidgetKind, &str, Option<Level>)> = registrations
            .contributions
            .of::<DashboardWidget>()
            .map(|widget| (widget.kind(), widget.func(), widget.level_value()))
            .collect();
        assert_eq!(
            widgets,
            [(
                WidgetKind::Stat,
                "keel.widget.accounts",
                Some(Level::Reseller)
            )]
        );
        let collected: Vec<(&str, u64, bool)> = registrations
            .contributions
            .of::<keel_sdk::StatCollector>()
            .map(|collector| {
                (
                    collector.key(),
                    collector.interval().as_secs(),
                    collector.is_per_account(),
                )
            })
            .collect();
        assert_eq!(
            collected,
            [
                ("keel.accounts", 3_600, false),
                ("keel.principals", 3_600, false)
            ]
        );
        let settings: Vec<(&str, Option<&Value>, Level)> = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| (def.key(), def.default_value(), def.level_value()))
            .collect();
        assert_eq!(
            settings,
            [("keel.idempotency.hours", Some(&json!(24)), Level::Admin)]
        );
        let schema = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| def.schema().as_value().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            schema,
            [json!({ "type": "integer", "minimum": 1, "maximum": 720 })]
        );
        let internal: Vec<&str> = registrations
            .functions
            .iter()
            .filter(|def| def.is_internal())
            .map(FunctionDef::name)
            .collect();
        assert_eq!(internal, [RESTART, IDEMPOTENCY_PRUNE]);
        assert!(registrations.menu.iter().all(|node| {
            ["access/", "monitor/", "system/"]
                .iter()
                .any(|group| node.path().starts_with(group))
        }));
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
