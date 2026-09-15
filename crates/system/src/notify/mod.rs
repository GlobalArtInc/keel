pub mod api;

mod counter;
mod entity;
mod functions;
mod lifecycle;
mod messages;
mod poll;
mod problems;

use std::sync::OnceLock;

use keel_sdk::prelude::*;
use keel_sdk::system::{Caller, LiveEvents, PrincipalSource, ProblemCounter};

pub use functions::internal::{MESSAGE_CREATE, PROBLEM_CLEAR, PROBLEM_RAISE};
pub use functions::{
    MESSAGE_CLEAR, MESSAGE_LIST, MESSAGE_MARK, PERMISSION_MANAGE as MANAGE,
    PERMISSION_READ as READ, PROBLEM_GET, PROBLEM_LIST,
};
pub use lifecycle::{Dropped, drop_account, on_delete};
pub use messages::Note;
pub use poll::{JOB_ID as POLL_JOB, SCHEDULE as POLL_SCHEDULE};

pub const FUNCTIONS: [&str; 5] = [
    MESSAGE_LIST,
    MESSAGE_MARK,
    MESSAGE_CLEAR,
    PROBLEM_LIST,
    PROBLEM_GET,
];
pub const INTERNAL_FUNCTIONS: [&str; 3] = [PROBLEM_RAISE, PROBLEM_CLEAR, MESSAGE_CREATE];
pub const MENU_PATH: &str = "system/problems";
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
    r.provide::<dyn ProblemCounter>(Arc::new(counter::Counter))
        .require::<dyn Caller>()
        .require::<dyn LiveEvents>()
        .require::<dyn PrincipalSource>()
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
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, PROBLEM_LIST)
                .icon("triangle-alert")
                .order(44),
        )
        .i18n("en", include_str!("../../i18n/notify/en.json"))
        .i18n("ru", include_str!("../../i18n/notify/ru.json"))
        .start(move |ctx| async move {
            let contributed: Vec<api::ProblemSource> =
                ctx.contributions::<api::ProblemSource>().cloned().collect();
            if started.set(contributed).is_err() {
                tracing::warn!("the notify module was started twice; the first sources stand");
            }
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
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), PROBLEM_LIST);
        assert!(registrations.provides.get::<dyn ProblemCounter>().is_some());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 3);
        assert!(required.iter().any(|name| name.ends_with("Caller")));
        assert!(required.iter().any(|name| name.ends_with("LiveEvents")));
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
