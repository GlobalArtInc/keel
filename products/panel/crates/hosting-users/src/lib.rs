pub mod api;
mod config;
mod files;
mod functions;
mod host;
mod lifecycle;
mod limit;
mod lookup;
mod user;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel::sdk::system::TaskRunner;
use keel_host::api::HostExecutor;

use api::HostingUsers;
use config::{HostingConfig, SECTION};
use lookup::Lookup;

const PRODUCT: &str = "panel";
const USER_PREFIX: &str = "hp_";
const FEATURE: &str = "panel.users";
const LIMIT: &str = "hosting.users";
const LIMIT_DEFAULT: u64 = 10;

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "task"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn HostExecutor>()
        .require::<dyn TaskRunner>()
        .provide::<dyn HostingUsers>(Arc::new(Lookup))
        .function(functions::list())
        .function(functions::get())
        .function(functions::create())
        .function(functions::edit())
        .function(functions::delete())
        .function(functions::options())
        .function(files::list())
        .function(files::mkdir())
        .function(files::rename())
        .function(files::delete())
        .function(files::chmod())
        .function(files::upload())
        .function(files::download())
        .function(files::apply())
        .view(
            files::VIEW,
            ViewSpec::new(Level::User)
                .permission(files::READ)
                .menu(
                    MenuNode::new("panel/files", files::VIEW)
                        .label("hosting.file-manager.title")
                        .icon("folder")
                        .order(15),
                )
                .needs([FEATURE]),
        )
        .permission("hosting.user.read", &[Level::User, Level::Reseller])
        .permission("hosting.user.manage", &[Level::User, Level::Reseller])
        .permission(files::READ, &[Level::User, Level::Reseller])
        .permission(files::MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/users", "hosting.user.list")
                .icon("users")
                .order(10),
        )
        .within("hosting.user.create", limit::enforce)
        .contribute(
            LimitDef::new(LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(lifecycle::contribution())
        .contribute(ConfigSection::new::<HostingConfig>(SECTION))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}
