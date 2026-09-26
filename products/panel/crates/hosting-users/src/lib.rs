mod functions;
mod host;
mod lifecycle;
mod limit;
mod user;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_host::api::HostExecutor;

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
    requires: &["host"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn HostExecutor>()
        .function(functions::list())
        .function(functions::get())
        .function(functions::create())
        .function(functions::edit())
        .function(functions::delete())
        .permission("hosting.user.read", &[Level::User, Level::Reseller])
        .permission("hosting.user.manage", &[Level::User, Level::Reseller])
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
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}
