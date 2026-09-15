pub mod api;

mod channels;
mod consumer;
mod counter;
mod entity;
mod functions;
mod lifecycle;
mod mailer;
mod messages;
mod poll;
mod problems;
mod sender;
mod templates;

use std::sync::OnceLock;

use keel_sdk::prelude::*;
use keel_sdk::system::{
    Caller, ContextExt, LiveEvents, OutboxSubscriber, PrincipalSource, ProblemCounter, SecretCipher,
};

pub use channels::{
    ERROR_CONFIG, ERROR_EMAIL_MISSING, ERROR_KIND, ERROR_KIND_UNAVAILABLE,
    email::TIMEOUT as SMTP_TIMEOUT,
};
pub use consumer::{BATCH as CONSUMER_BATCH, CURSOR, STATE_FAILED, STATE_PENDING, STATE_SENT};
pub use functions::channels::{
    CHANNEL_DELETE, CHANNEL_GET, CHANNEL_LIST, CHANNEL_SET, CHANNEL_TEST,
};
pub use functions::deliveries::DELIVERY_LIST;
pub use functions::internal::{MESSAGE_CREATE, PROBLEM_CLEAR, PROBLEM_RAISE};
pub use functions::{
    MESSAGE_CLEAR, MESSAGE_LIST, MESSAGE_MARK, PERMISSION_MANAGE as MANAGE,
    PERMISSION_READ as READ, PROBLEM_GET, PROBLEM_LIST,
};
pub use lifecycle::{Dropped, drop_account, on_delete};
pub use messages::Note;
pub use poll::{JOB_ID as POLL_JOB, SCHEDULE as POLL_SCHEDULE};
pub use sender::{
    LADDER, MAX_ATTEMPTS, POLL as SENDER_POLL, PROBLEM_KEY as DELIVERY_FAILED,
    PROBLEM_SOURCE as DELIVERY_SOURCE,
};
pub use templates::{
    BLOCK_DELIMITERS, COMMENT_DELIMITERS, ERROR_PLACEHOLDER as ERROR_TEMPLATE_PLACEHOLDER,
    ERROR_SYNTAX as ERROR_TEMPLATE_SYNTAX, ERROR_TWICE as ERROR_TEMPLATE_TWICE,
    VARIABLE_DELIMITERS, placeholder_fault, shipped as shipped_templates,
};

pub const FUNCTIONS: [&str; 11] = [
    MESSAGE_LIST,
    MESSAGE_MARK,
    MESSAGE_CLEAR,
    PROBLEM_LIST,
    PROBLEM_GET,
    CHANNEL_LIST,
    CHANNEL_GET,
    CHANNEL_SET,
    CHANNEL_DELETE,
    CHANNEL_TEST,
    DELIVERY_LIST,
];
pub const INTERNAL_FUNCTIONS: [&str; 3] = [PROBLEM_RAISE, PROBLEM_CLEAR, MESSAGE_CREATE];
pub const MENU_PATH: &str = "system/problems";
pub const CHANNELS_MENU_PATH: &str = "system/channels";
pub const WIDGET_LIMIT: u32 = 5;

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/notify").table("keel_migration_notify");

pub static MODULE: ModuleDef = ModuleDef {
    id: "notify",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["cron"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let sources: poll::Sources = Arc::new(OnceLock::new());
    let started = sources.clone();
    let mailer = Arc::new(mailer::Mailer::new());
    let mailing = mailer.clone();
    r.provide::<dyn ProblemCounter>(Arc::new(counter::Counter))
        .require::<dyn Caller>()
        .require::<dyn LiveEvents>()
        .require::<dyn PrincipalSource>()
        .require::<dyn SecretCipher>()
        .require::<dyn OutboxSubscriber>()
        .contribute(poll::job(sources))
        .contribute(DashboardWidget::list(PROBLEM_LIST, WIDGET_LIMIT).level(Level::Admin))
        .function(functions::internal::problem_raise())
        .function(functions::internal::problem_clear())
        .function(functions::internal::message_create())
        .function(functions::message_list())
        .function(functions::message_mark())
        .function(functions::message_clear())
        .function(functions::problem_list())
        .function(functions::problem_get())
        .function(functions::channels::channel_list())
        .function(functions::channels::channel_get())
        .function(functions::channels::channel_set())
        .function(functions::channels::channel_delete())
        .function(functions::channels::channel_test(mailer.clone()))
        .function(functions::deliveries::delivery_list())
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, PROBLEM_LIST)
                .icon("triangle-alert")
                .order(44),
        )
        .menu(
            MenuNode::new(CHANNELS_MENU_PATH, CHANNEL_LIST)
                .icon("mail")
                .order(46),
        )
        .i18n("en", include_str!("../../i18n/notify/en.json"))
        .i18n("ru", include_str!("../../i18n/notify/ru.json"))
        .start(move |ctx| async move {
            let contributed: Vec<api::ProblemSource> =
                ctx.contributions::<api::ProblemSource>().cloned().collect();
            if started.set(contributed).is_err() {
                tracing::warn!("the notify module was started twice; the first sources stand");
            }
            let _ = rustls::crypto::ring::default_provider().install_default();
            let shipped = templates::shipped();
            let templates = shipped
                .iter()
                .chain(ctx.contributions::<api::MailTemplate>());
            let mail = mailing.start(
                mailer::Seams {
                    db: ctx.db(),
                    clock: ctx.require::<dyn Clock>()?,
                    cipher: ctx.require::<dyn SecretCipher>()?,
                    caller: ctx.require::<dyn Caller>()?,
                    principals: ctx.require::<dyn PrincipalSource>()?,
                    subscriber: ctx.require::<dyn OutboxSubscriber>()?,
                },
                templates,
            )?;
            ctx.spawn(consumer::run(mail.clone(), mailing.clone()));
            ctx.spawn(sender::run(mail, mailing));
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_functions_hides_the_internal_ones_and_contributes_the_poll_job() {
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
                    def.is_internal(),
                )
            })
            .collect();
        assert_eq!(
            functions,
            [
                (PROBLEM_RAISE, Kind::Action, Mode::Write, Level::Admin, true),
                (PROBLEM_CLEAR, Kind::Action, Mode::Write, Level::Admin, true),
                (
                    MESSAGE_CREATE,
                    Kind::Action,
                    Mode::Write,
                    Level::Admin,
                    true
                ),
                (MESSAGE_LIST, Kind::List, Mode::Read, Level::User, false),
                (MESSAGE_MARK, Kind::Action, Mode::Write, Level::User, false),
                (MESSAGE_CLEAR, Kind::Action, Mode::Write, Level::User, false),
                (PROBLEM_LIST, Kind::List, Mode::Read, Level::User, false),
                (PROBLEM_GET, Kind::Get, Mode::Read, Level::User, false),
                (CHANNEL_LIST, Kind::List, Mode::Read, Level::Admin, false),
                (CHANNEL_GET, Kind::Get, Mode::Read, Level::Admin, false),
                (CHANNEL_SET, Kind::Form, Mode::Write, Level::Admin, false),
                (
                    CHANNEL_DELETE,
                    Kind::Action,
                    Mode::Write,
                    Level::Admin,
                    false
                ),
                (CHANNEL_TEST, Kind::Action, Mode::Write, Level::Admin, false),
                (DELIVERY_LIST, Kind::List, Mode::Read, Level::Admin, false),
            ]
        );
        let public: Vec<&str> = functions
            .iter()
            .filter(|(.., internal)| !internal)
            .map(|(name, ..)| *name)
            .collect();
        assert_eq!(public, FUNCTIONS);
        let internal: Vec<&str> = functions
            .iter()
            .filter(|(.., internal)| *internal)
            .map(|(name, ..)| *name)
            .collect();
        assert_eq!(internal, INTERNAL_FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            if def.is_internal() {
                assert!(def.ui().is_none(), "{} has no screen", def.name());
                assert_eq!(def.access().permission, None);
                assert!(def.owned().is_none());
            }
        }
        let owned: Vec<&str> = registrations
            .functions
            .iter()
            .filter(|def| def.owned().is_some())
            .map(FunctionDef::name)
            .collect();
        assert_eq!(owned, [MESSAGE_LIST, MESSAGE_MARK]);
        let permissions: Vec<(&str, &[Level])> = registrations
            .permissions
            .iter()
            .map(|def| (def.key.as_str(), def.defaults.as_slice()))
            .collect();
        assert_eq!(
            permissions,
            [
                (READ, &[Level::User, Level::Reseller][..]),
                (MANAGE, &[Level::User, Level::Reseller][..]),
            ]
        );
        assert_eq!(registrations.menu.len(), 2);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), PROBLEM_LIST);
        assert_eq!(registrations.menu[1].path(), CHANNELS_MENU_PATH);
        assert_eq!(registrations.menu[1].func(), CHANNEL_LIST);
        assert!(registrations.provides.get::<dyn ProblemCounter>().is_some());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 5);
        assert!(required.iter().any(|name| name.ends_with("Caller")));
        assert!(required.iter().any(|name| name.ends_with("LiveEvents")));
        assert!(required.iter().any(|name| name.ends_with("SecretCipher")));
        assert!(
            required
                .iter()
                .any(|name| name.ends_with("OutboxSubscriber"))
        );
        assert!(
            required
                .iter()
                .any(|name| name.ends_with("PrincipalSource"))
        );
        let jobs: Vec<(&str, &str)> = registrations
            .contributions
            .of::<super::super::cron::api::CronJob>()
            .map(|job| (job.id(), job.schedule()))
            .collect();
        assert_eq!(jobs, [(POLL_JOB, POLL_SCHEDULE)]);
        let widgets: Vec<(&str, Option<u32>)> = registrations
            .contributions
            .of::<DashboardWidget>()
            .map(|widget| (widget.func(), widget.limit_value()))
            .collect();
        assert_eq!(widgets, [(PROBLEM_LIST, Some(WIDGET_LIMIT))]);
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_none());
        assert_eq!(MODULE.id, "notify");
        assert_eq!(MODULE.requires, ["cron"]);
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_notify")
        );
    }
}
