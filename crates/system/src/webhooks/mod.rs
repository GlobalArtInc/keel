mod consumer;
mod entity;
mod functions;
mod lifecycle;
mod masks;
mod sender;
mod signature;
mod started;
mod targets;

use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_sdk::system::{Caller, ContextExt, OutboxSubscriber, PrincipalSource, SecretCipher};

pub use consumer::{CURSOR, STATE_DEAD, STATE_DELIVERED, STATE_FAILED, STATE_PENDING};
pub use functions::PERMISSION_MANAGE as MANAGE;
pub use functions::deliveries::{DELIVERY_GET, DELIVERY_LIST, DELIVERY_RETRY, ERROR_NOT_RETRYABLE};
pub use functions::subscriptions::{
    ERROR_MASK, ERROR_SECRET_SHORT, SECRET_GENERATED_BYTES, SECRET_MIN_BYTES, SUBSCRIPTION_DELETE,
    SUBSCRIPTION_GET, SUBSCRIPTION_LIST, SUBSCRIPTION_SET, SUBSCRIPTION_TOGGLE, base64url,
};
pub use lifecycle::{Dropped, drop_account};
pub use sender::{LADDER, MAX_ATTEMPTS, PROBLEM_KEY as DEAD_LETTER};
pub use signature::{
    EVENT_ID_HEADER, HEADER as SIGNATURE_HEADER, Rejection, WINDOW_SECONDS, sign, verify,
};
pub use started::PRIVATE_TARGETS_KEY;
pub use targets::{ERROR_URL_PRIVATE, Resolver};

pub const FUNCTIONS: [&str; 8] = [
    SUBSCRIPTION_LIST,
    SUBSCRIPTION_GET,
    SUBSCRIPTION_SET,
    SUBSCRIPTION_TOGGLE,
    SUBSCRIPTION_DELETE,
    DELIVERY_LIST,
    DELIVERY_GET,
    DELIVERY_RETRY,
];
pub const MENU_PATH: &str = "system/webhooks";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/webhooks").table("keel_migration_webhook");

pub static MODULE: ModuleDef = ModuleDef {
    id: "webhook",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["notify"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn Caller>()
        .require::<dyn PrincipalSource>()
        .require::<dyn SecretCipher>()
        .require::<dyn OutboxSubscriber>()
        .require::<dyn SettingsSource>()
        .contribute(
            SettingDef::new(PRIVATE_TARGETS_KEY, schema_for!(bool))
                .default(false)
                .level(Level::Admin),
        )
        .function(functions::subscriptions::subscription_list())
        .function(functions::subscriptions::subscription_get())
        .function(functions::subscriptions::subscription_set())
        .function(functions::subscriptions::subscription_toggle())
        .function(functions::subscriptions::subscription_delete())
        .function(functions::deliveries::delivery_list())
        .function(functions::deliveries::delivery_get())
        .function(functions::deliveries::delivery_retry())
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, SUBSCRIPTION_LIST)
                .icon("share-2")
                .order(48),
        )
        .i18n("en", include_str!("../../i18n/webhooks/en.json"))
        .i18n("ru", include_str!("../../i18n/webhooks/ru.json"))
        .start(|ctx| async move {
            let started = started::Started::new(started::Seams {
                db: ctx.db(),
                clock: ctx.require::<dyn Clock>()?,
                cipher: ctx.require::<dyn SecretCipher>()?,
                caller: ctx.require::<dyn Caller>()?,
                principals: ctx.require::<dyn PrincipalSource>()?,
                subscriber: ctx.require::<dyn OutboxSubscriber>()?,
                settings: ctx.require::<dyn SettingsSource>()?,
                resolver: targets::resolver_or_system(ctx.require::<dyn Resolver>().ok()),
            });
            ctx.spawn(consumer::run(started.clone()));
            ctx.spawn(sender::run(started));
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_functions_its_setting_its_permission_and_requires_the_seams() {
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
                (SUBSCRIPTION_LIST, Kind::List, Mode::Read, Level::User, true),
                (SUBSCRIPTION_GET, Kind::Get, Mode::Read, Level::User, true),
                (SUBSCRIPTION_SET, Kind::Form, Mode::Write, Level::User, true),
                (
                    SUBSCRIPTION_TOGGLE,
                    Kind::Action,
                    Mode::Write,
                    Level::User,
                    true
                ),
                (
                    SUBSCRIPTION_DELETE,
                    Kind::Action,
                    Mode::Write,
                    Level::User,
                    true
                ),
                (DELIVERY_LIST, Kind::List, Mode::Read, Level::User, true),
                (DELIVERY_GET, Kind::Get, Mode::Read, Level::User, true),
                (DELIVERY_RETRY, Kind::Action, Mode::Write, Level::User, true),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert_eq!(
                def.access().permission.as_deref(),
                Some(MANAGE),
                "{}",
                def.name()
            );
        }
        let permissions: Vec<(&str, &[Level])> = registrations
            .permissions
            .iter()
            .map(|def| (def.key.as_str(), def.defaults.as_slice()))
            .collect();
        assert_eq!(permissions, [(MANAGE, &[Level::User, Level::Reseller][..])]);
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), SUBSCRIPTION_LIST);
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 5);
        for seam in [
            "Caller",
            "PrincipalSource",
            "SecretCipher",
            "OutboxSubscriber",
            "SettingsSource",
        ] {
            assert!(
                required.iter().any(|name| name.ends_with(seam)),
                "{seam} in {required:?}"
            );
        }
        let settings: Vec<(&str, Option<&Value>, Level)> = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| (def.key(), def.default_value(), def.level_value()))
            .collect();
        assert_eq!(
            settings,
            [(PRIVATE_TARGETS_KEY, Some(&json!(false)), Level::Admin)]
        );
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_none());
        assert_eq!(MODULE.id, "webhook");
        assert_eq!(MODULE.requires, ["notify"]);
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_webhook")
        );
    }
}
