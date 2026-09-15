pub mod api;

mod entity;
mod leader;
mod log;
mod runner;
mod sink;
mod worker;

use keel_sdk::prelude::*;
use keel_sdk::system::{AdvisoryLock, Caller, ContextExt, LiveEvents, TaskRunner};

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
        .i18n("en", include_str!("../../i18n/tasks/en.json"))
        .i18n("ru", include_str!("../../i18n/tasks/ru.json"))
        .start(move |ctx| async move {
            let shared = started.start(
                ctx.db(),
                ctx.require::<dyn Clock>()?,
                ctx.require::<dyn LiveEvents>()?,
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
        assert!(registrations.functions.is_empty());
        assert!(registrations.provides.get::<dyn TaskRunner>().is_some());
        assert!(registrations.provides.get::<api::Drain>().is_some());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 3);
        assert!(required.iter().any(|name| name.ends_with("Caller")));
        assert!(required.iter().any(|name| name.ends_with("AdvisoryLock")));
        assert!(required.iter().any(|name| name.ends_with("LiveEvents")));
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
