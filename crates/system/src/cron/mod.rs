pub mod api;

mod entity;
mod functions;
mod leader;
mod prune;
mod schedule;
mod scheduler;

use std::collections::BTreeMap;

use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_sdk::system::{AdvisoryLock, Caller, ContextExt};

pub use api::{ERROR_NOT_LEADER, ERROR_RUNNING, ERROR_UNKNOWN_JOB, OWNER_LOGIN};
pub use functions::{JOB_LIST, JOB_RUN, RUN_LIST};
pub use leader::LOCK_KEY;
pub use prune::{
    FUNCTION as IDEMPOTENCY_PRUNE, JOB_ID as IDEMPOTENCY_JOB, SCHEDULE as IDEMPOTENCY_SCHEDULE,
};
pub use schedule::ERROR_SCHEDULE;
pub use scheduler::{ERROR_NOT_RUNNING, OVERRIDES_KEY, TICK};

pub const FUNCTIONS: [&str; 3] = [JOB_LIST, JOB_RUN, RUN_LIST];
pub const MENU_PATH: &str = "system/cron";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/cron").table("keel_migration_cron");

pub static MODULE: ModuleDef = ModuleDef {
    id: "cron",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let core = Arc::new(scheduler::Core::new());
    let started = core.clone();
    let stopped = core.clone();
    r.provide::<api::Scheduler>(Arc::new(api::Scheduler::new(core)))
        .require::<dyn Caller>()
        .require::<dyn AdvisoryLock>()
        .require::<dyn SettingsSource>()
        .contribute(
            SettingDef::new(OVERRIDES_KEY, schema_for!(BTreeMap<String, String>))
                .default(json!({}))
                .level(Level::Admin),
        )
        .contribute(prune::job())
        .function(functions::job_list())
        .function(functions::job_run())
        .function(functions::run_list())
        .menu(MenuNode::new(MENU_PATH, JOB_LIST).icon("timer").order(42))
        .i18n("en", include_str!("../../i18n/cron/en.json"))
        .i18n("ru", include_str!("../../i18n/cron/ru.json"))
        .start(move |ctx| async move {
            let shared = started.start(
                ctx.db(),
                ctx.require::<dyn Clock>()?,
                ctx.require::<dyn Caller>()?,
                ctx.require::<dyn SettingsSource>()?,
                ctx.clone(),
                ctx.contributions::<api::CronJob>(),
            )?;
            let lock = ctx.require::<dyn AdvisoryLock>()?;
            ctx.spawn(scheduler::lead(shared, lock));
            Ok(())
        })
        .stop(move || async move {
            match stopped.started() {
                Ok(shared) => shared.stop().await,
                Err(_) => Ok(()),
            }
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_provides_the_scheduler_contributes_its_overrides_and_the_prune_job_and_requires_the_seams()
     {
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
                ("cron.job.list", Kind::List, Mode::Read, Level::Admin),
                ("cron.job.run", Kind::Action, Mode::Write, Level::Admin),
                ("cron.run.list", Kind::List, Mode::Read, Level::Admin),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert_eq!(def.key(), Some("id"));
            assert!(def.owned().is_none());
            assert_eq!(def.access().permission, None);
        }
        assert!(registrations.permissions.is_empty());
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), JOB_LIST);
        assert!(registrations.provides.get::<api::Scheduler>().is_some());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 3);
        assert!(required.iter().any(|name| name.ends_with("Caller")));
        assert!(required.iter().any(|name| name.ends_with("AdvisoryLock")));
        assert!(required.iter().any(|name| name.ends_with("SettingsSource")));
        let settings: Vec<&SettingDef> = registrations.contributions.of::<SettingDef>().collect();
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0].key(), OVERRIDES_KEY);
        assert_eq!(settings[0].module(), MODULE.id);
        assert_eq!(settings[0].level_value(), Level::Admin);
        assert_eq!(settings[0].default_value(), Some(&json!({})));
        assert_eq!(
            settings[0].schema().get("additionalProperties"),
            Some(&json!({ "type": "string" }))
        );
        let jobs: Vec<(&str, &str, &str)> = registrations
            .contributions
            .of::<api::CronJob>()
            .map(|job| (job.id(), job.schedule(), job.module()))
            .collect();
        assert_eq!(jobs, [(IDEMPOTENCY_JOB, IDEMPOTENCY_SCHEDULE, MODULE.id)]);
        assert_eq!(IDEMPOTENCY_JOB, "cron.idempotency.prune");
        assert_eq!(IDEMPOTENCY_SCHEDULE, "25 * * * *");
        assert_eq!(IDEMPOTENCY_PRUNE, "keel.idempotency.prune");
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_some());
        assert_eq!(MODULE.id, "cron");
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_cron")
        );
        assert_eq!(TICK.as_secs(), 1);
    }
}
