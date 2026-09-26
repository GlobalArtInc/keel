pub mod api;
mod config;
mod database;
mod databases;
mod grant;
mod host;
mod lifecycle;
mod limit;
mod login;
mod logins;
mod lookup;
mod names;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_host::api::HostExecutor;
use keel_hosting_users::api::HostingUsers;

use config::{DatabaseConfig, SECTION};
use host::Dumps;
use lookup::Lookup;

const PRODUCT: &str = "panel";
const USER_PREFIX: &str = "hp_";
const FEATURE: &str = "panel.databases";
const DATABASE_LIMIT: &str = "hosting.database.max";
const LOGIN_LIMIT: &str = "hosting.database.user.max";
const LIMIT_DEFAULT: u64 = 10;
const BACKUP: &str = "hosting_database.dumps";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_database");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_database",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "hosting"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let dumps = Arc::new(Dumps::default());
    let started = Arc::clone(&dumps);
    r.require::<dyn HostExecutor>()
        .require::<dyn HostingUsers>()
        .provide::<dyn api::Databases>(Arc::new(Lookup))
        .function(databases::list())
        .function(databases::create())
        .function(databases::delete())
        .function(databases::options())
        .function(logins::list())
        .function(logins::get())
        .function(logins::create())
        .function(logins::edit())
        .function(logins::delete())
        .permission(databases::READ, &[Level::User, Level::Reseller])
        .permission(databases::MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/databases", "hosting.database.list")
                .icon("database")
                .order(40),
        )
        .menu(
            MenuNode::new("panel/database-users", "hosting.database.user.list")
                .icon("key")
                .order(41),
        )
        .within(databases::CREATE, limit::databases)
        .within(logins::CREATE, limit::logins)
        .within("hosting.user.delete", limit::refuse_busy_users)
        .contribute(
            LimitDef::new(DATABASE_LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(
            LimitDef::new(LOGIN_LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(ConfigSection::new::<DatabaseConfig>(SECTION))
        .contribute(lifecycle::contribution())
        .contribute(BackupSource::new(BACKUP, move |dir: BackupDir| {
            let dumps = Arc::clone(&dumps);
            async move { dumps.write(&dir).await }
        }))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
