mod expand;
mod functions;
mod record;
mod template;

use keel::prelude::*;
use keel_dns_zones::api::ZoneTemplates;

use expand::Expander;

const PLATFORM: Id = Id::ROOT_ACCOUNT;

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_dns_template");

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns_template",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["dns", "dns_record"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn ZoneTemplates>(Arc::new(Expander))
        .function(functions::list())
        .function(functions::get())
        .function(functions::create())
        .function(functions::edit())
        .function(functions::delete())
        .function(functions::options())
        .permission("dns.template.read", &[Level::User, Level::Reseller])
        .permission("dns.template.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("dns/templates", "dns.template.list")
                .icon("layers")
                .order(20),
        )
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}
