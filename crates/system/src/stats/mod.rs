mod collectors;
mod functions;
mod rollup;
mod sample;
mod time;

use keel_sdk::prelude::*;

use collectors::Stats;

pub use functions::{ERROR_KEY, ERROR_SUBJECT};
pub use rollup::{AGGREGATE_JOB, HOUR_DAYS_KEY, PRUNE_JOB, RAW_DAYS_KEY};
pub use sample::JOB_ID as COLLECT_JOB;

pub const KEYS: &str = "stats.keys";
pub const SERIES: &str = "stats.series";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/stats").table("keel_migration_stats");

pub static MODULE: ModuleDef = ModuleDef {
    id: "stats",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["cron"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let stats = Arc::new(Stats::default());
    let started = stats.clone();
    r.require::<dyn SettingsSource>()
        .contribute(
            SettingDef::new(RAW_DAYS_KEY, rollup::days_schema())
                .default(rollup::RAW_DAYS)
                .level(Level::Admin),
        )
        .contribute(
            SettingDef::new(HOUR_DAYS_KEY, rollup::days_schema())
                .default(rollup::HOUR_DAYS)
                .level(Level::Admin),
        )
        .contribute(sample::job(stats.clone()))
        .contribute(rollup::aggregate_job())
        .contribute(rollup::prune_job(stats.clone()))
        .function(functions::keys(stats.clone()))
        .function(functions::series(stats))
        .i18n("en", include_str!("../../i18n/stats/en.json"))
        .i18n("ru", include_str!("../../i18n/stats/ru.json"))
        .start(move |ctx| async move { started.start(&ctx) });
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::cron::api::CronJob;

    use super::*;

    #[test]
    fn the_module_registers_its_two_reads_three_jobs_and_two_retention_settings() {
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
                (KEYS, Kind::Get, Mode::Read, Level::User),
                (SERIES, Kind::Get, Mode::Read, Level::User),
            ]
        );
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert!(def.owned().is_none());
            assert_eq!(def.access().permission, None);
            assert!(!def.is_internal());
        }
        let jobs: Vec<(&str, &str)> = registrations
            .contributions
            .of::<CronJob>()
            .map(|job| (job.id(), job.schedule()))
            .collect();
        assert_eq!(
            jobs,
            [
                (COLLECT_JOB, sample::SCHEDULE),
                (AGGREGATE_JOB, rollup::AGGREGATE_SCHEDULE),
                (PRUNE_JOB, rollup::PRUNE_SCHEDULE),
            ]
        );
        let settings: Vec<(&str, Option<&Value>, Level)> = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| (def.key(), def.default_value(), def.level_value()))
            .collect();
        assert_eq!(
            settings,
            [
                (RAW_DAYS_KEY, Some(&json!(7)), Level::Admin),
                (HOUR_DAYS_KEY, Some(&json!(90)), Level::Admin),
            ]
        );
        assert!(registrations.menu.is_empty());
        assert_eq!(MODULE.requires, ["cron"]);
        assert!(MODULE.migrations.is_some());
    }
}
