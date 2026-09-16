pub mod api;

mod download;
mod entity;
mod functions;
mod leader;
mod log;
mod runner;
mod sink;
mod worker;

use keel_sdk::prelude::*;
use keel_sdk::system::{AdvisoryLock, Caller, ContextExt, LiveEvents, ParamMasker, TaskRunner};

pub use download::{ERROR_NO_FILE, FUNCTION as DOWNLOAD_FUNCTION};
pub use functions::{LOG_TAIL, PERMISSION_MANAGE as MANAGE, PERMISSION_READ as READ};
pub use leader::LOCK_KEY;
pub use log::{
    DIRECTORY as LOG_DIRECTORY, LOG_EXTENSION, OUTPUT_EXTENSION, log_path, output_path,
    tail as log_tail,
};
pub use runner::{
    ERROR_FINISHED, ERROR_INTERRUPTED, ERROR_NOT_FOUND, ERROR_NOT_LOCAL, ERROR_NOT_RUNNING,
    FRAME_KIND,
};
pub use worker::{POLL, STOP_GRACE};

pub const FUNCTIONS: [&str; 4] = ["task.list", "task.get", "task.cancel", "task.download"];
pub const MENU_PATH: &str = "system/tasks";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/tasks").table("keel_migration_task");

pub static MODULE: ModuleDef = ModuleDef {
    id: "task",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let runner = Arc::new(runner::Runner::new());
    let provided: Arc<dyn TaskRunner> = runner.clone();
    let started = runner.clone();
    let stopped = runner.clone();
    r.provide::<dyn TaskRunner>(provided)
        .provide::<api::Drain>(Arc::new(api::Drain::new(runner)))
        .require::<dyn Caller>()
        .require::<dyn AdvisoryLock>()
        .require::<dyn LiveEvents>()
        .require::<dyn ParamMasker>()
        .function(functions::list())
        .function(functions::get())
        .function(functions::cancel())
        .function(download::download())
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new(MENU_PATH, "task.list")
                .icon("list-checks")
                .order(40),
        )
        .i18n("en", include_str!("../../i18n/tasks/en.json"))
        .i18n("ru", include_str!("../../i18n/tasks/ru.json"))
        .start(move |ctx| async move {
            let shared = started.start(
                ctx.db(),
                ctx.require::<dyn Clock>()?,
                ctx.require::<dyn LiveEvents>()?,
                ctx.require::<dyn ParamMasker>()?,
                ctx.data_dir(),
            );
            let worker = worker::Worker::new(
                shared,
                ctx.clone(),
                ctx.require::<dyn Caller>()?,
                ctx.require::<dyn AdvisoryLock>()?,
            );
            ctx.spawn(worker.run());
            Ok(())
        })
        .stop(move || async move {
            match stopped.shared() {
                Ok(shared) => worker::drain(shared, STOP_GRACE).await,
                Err(_) => Ok(()),
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_provides_the_runner_and_the_drain_and_requires_the_kernel_seams() {
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
                ("task.list", Kind::List, Mode::Read, Level::User),
                ("task.get", Kind::Get, Mode::Read, Level::User),
                ("task.cancel", Kind::Action, Mode::Write, Level::User),
                ("task.download", Kind::Download, Mode::Read, Level::User),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert_eq!(def.key(), Some("id"));
            assert_eq!(
                def.owned().map(|owned| owned.table.as_str()),
                Some("task_job")
            );
            let permission = if def.name() == "task.cancel" {
                MANAGE
            } else {
                READ
            };
            assert_eq!(def.access().permission.as_deref(), Some(permission));
        }
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
        assert_eq!(registrations.menu[0].func(), "task.list");
        assert!(registrations.provides.get::<dyn TaskRunner>().is_some());
        assert!(registrations.provides.get::<api::Drain>().is_some());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 4);
        assert!(required.iter().any(|name| name.ends_with("Caller")));
        assert!(required.iter().any(|name| name.ends_with("AdvisoryLock")));
        assert!(required.iter().any(|name| name.ends_with("LiveEvents")));
        assert!(required.iter().any(|name| name.ends_with("ParamMasker")));
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_some());
        assert_eq!(MODULE.id, "task");
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_task")
        );
        assert_eq!(STOP_GRACE.as_secs(), 20);
    }
}
