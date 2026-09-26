mod backup;
mod config;
mod functions;
mod lifecycle;
mod names;
mod ops;
mod run;
mod schedule;
mod schedules;
mod seal;
mod tree;

use keel::prelude::*;
use keel_host::api::HostExecutor;
use keel_hosting_db::api::Databases;
use keel_hosting_mail::api::Mailboxes;
use keel_hosting_web::api::Sites;
use keel_system::cron::api::CronJob;

use config::{BackupConfig, SECTION};

const PRODUCT: &str = "panel";
const FEATURE: &str = "panel.backup";
const READ: &str = "hosting.backup.read";
const MANAGE: &str = "hosting.backup.manage";
const CLASS: &str = "hosting.backup";
const STATE_PENDING: &str = "pending";
const STATE_ACTIVE: &str = "active";
const STATE_ERROR: &str = "error";
const KEEP_DEFAULT: usize = 7;
const KEEP_MAX: u32 = 30;
const SITE_DELETE: &str = "hosting.site.delete";
const SCHEDULED_JOB: &str = "hosting.backup.scheduled";
const SCHEDULED_EVERY: &str = "0 * * * *";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_backup");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_backup",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[
        "host",
        "hosting_site",
        "hosting_database",
        "hosting_mail",
        "task",
        "cron",
    ],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn HostExecutor>()
        .require::<dyn Sites>()
        .require::<dyn Databases>()
        .require::<dyn Mailboxes>()
        .function(functions::list())
        .function(functions::create())
        .function(functions::restore())
        .function(functions::delete())
        .function(functions::download())
        .function(functions::sites())
        .function(functions::scheduled_run())
        .function(functions::due())
        .function(schedules::list())
        .function(schedules::get())
        .function(schedules::create())
        .function(schedules::edit())
        .function(schedules::delete())
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/backups", functions::LIST)
                .icon("archive")
                .order(70),
        )
        .menu(
            MenuNode::new("panel/backup-schedules", "hosting.backup.schedule.list")
                .icon("calendar")
                .order(71),
        )
        .within(SITE_DELETE, lifecycle::forget_sites)
        .contribute(lifecycle::contribution())
        .contribute(ConfigSection::new::<BackupConfig>(SECTION))
        .contribute(ops::pack_op())
        .contribute(ops::unpack_op())
        .contribute(CronJob::new(
            SCHEDULED_JOB,
            SCHEDULED_EVERY,
            |run| async move { run.invoke(functions::DUE, json!({})).await.map(drop) },
        ))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}
