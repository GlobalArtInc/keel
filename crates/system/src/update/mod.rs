mod apply;
mod channel;
mod config;
mod dump;
mod entity;
mod history;
mod poll;
mod request;
mod status;

use std::sync::OnceLock;

use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_sdk::system::{ContextExt, TaskRunner};
use keel_sdk::types::Platform;

use super::tasks::api::Drain;

use channel::Channel;
use config::UpdateConfig;

pub use apply::{
    ERROR_CONTRACT, ERROR_CONTRACT_UNKNOWN, ERROR_MIN_UPGRADE_FROM, ERROR_NO_PREVIOUS,
    ERROR_UP_TO_DATE,
};
pub use channel::{ERROR_CHANNEL, ERROR_MANIFEST};
pub use config::{CHANNEL_URL, ERROR_CHANNEL_URL, SECTION};
pub use dump::{
    DIRECTORY as BACKUPS_DIRECTORY, MANIFEST as DUMP_MANIFEST, TABLE_EXTENSION as DUMP_EXTENSION,
};
pub use poll::{JOB_ID as POLL_JOB, PROBLEM_KEY, SCHEDULE as POLL_SCHEDULE};
pub use request::FILE as REQUEST_FILE;

pub const CHECK: &str = "update.check";
pub const APPLY: &str = "update.apply";
pub const ROLLBACK: &str = "update.rollback";
pub const FUNCTIONS: [&str; 3] = [CHECK, APPLY, ROLLBACK];
pub const CHANNEL_KEY: &str = "update.channel";
pub const MENU_PATH: &str = "system/updates";
pub const MENU_ICON: &str = "package";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/update").table("keel_migration_update");

pub static MODULE: ModuleDef = ModuleDef {
    id: "update",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["task", "cron"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let pool: apply::Pool = Arc::new(OnceLock::new());
    let started = pool.clone();
    r.require::<dyn TaskRunner>()
        .require::<dyn SettingsSource>()
        .require::<Drain>()
        .require::<Platform>()
        .contribute(ConfigSection::new::<UpdateConfig>(SECTION))
        .contribute(
            SettingDef::new(CHANNEL_KEY, schema_for!(Channel))
                .default(json!(Channel::Stable.as_str()))
                .level(Level::Admin),
        )
        .contribute(poll::job())
        .function(status::check())
        .function(apply::apply(pool.clone()))
        .function(apply::rollback(pool))
        .menu(MenuNode::new(MENU_PATH, CHECK).icon(MENU_ICON).order(48))
        .i18n("en", include_str!("../../i18n/update/en.json"))
        .i18n("ru", include_str!("../../i18n/update/ru.json"))
        .start(move |ctx| async move {
            ctx.config::<UpdateConfig>()?.base()?;
            if started.set(ctx.db()).is_err() {
                tracing::warn!("the update module was started twice; the first pool stands");
            }
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_screen_its_two_tasks_the_channel_setting_and_the_daily_job() {
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
                (CHECK, Kind::Get, Mode::Read, Level::Admin),
                (APPLY, Kind::Action, Mode::Write, Level::Admin),
                (ROLLBACK, Kind::Action, Mode::Write, Level::Admin),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert!(def.owned().is_none());
            assert_eq!(def.access().permission, None);
            assert!(!def.is_internal());
        }
        assert!(registrations.permissions.is_empty());
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), CHECK);
        assert_eq!(registrations.menu[0].icon_name(), Some(MENU_ICON));
        assert!(registrations.provides.is_empty());
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert_eq!(required.len(), 4);
        assert!(required.iter().any(|name| name.ends_with("TaskRunner")));
        assert!(required.iter().any(|name| name.ends_with("SettingsSource")));
        assert!(required.iter().any(|name| name.ends_with("Drain")));
        assert!(required.iter().any(|name| name.ends_with("Platform")));
        let settings: Vec<&SettingDef> = registrations.contributions.of::<SettingDef>().collect();
        assert_eq!(settings.len(), 1);
        assert_eq!(settings[0].key(), CHANNEL_KEY);
        assert_eq!(settings[0].module(), MODULE.id);
        assert_eq!(settings[0].level_value(), Level::Admin);
        assert_eq!(settings[0].default_value(), Some(&json!("stable")));
        assert_eq!(
            settings[0].schema().get("enum"),
            Some(&json!(["stable", "beta"]))
        );
        let sections: Vec<&str> = registrations
            .contributions
            .of::<ConfigSection>()
            .map(ConfigSection::key)
            .collect();
        assert_eq!(sections, [SECTION]);
        let jobs: Vec<(&str, &str)> = registrations
            .contributions
            .of::<super::super::cron::api::CronJob>()
            .map(|job| (job.id(), job.schedule()))
            .collect();
        assert_eq!(jobs, [(POLL_JOB, POLL_SCHEDULE)]);
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert!(registrations.start.is_some());
        assert!(registrations.stop.is_none());
        assert_eq!(MODULE.id, "update");
        assert_eq!(MODULE.requires, ["task", "cron"]);
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_update")
        );
        assert_eq!(CHANNEL_URL, "https://dl.globalart.hu/keel/");
        assert_eq!(REQUEST_FILE, "update.request");
        assert_eq!(BACKUPS_DIRECTORY, "backups");
    }
}
