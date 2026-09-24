mod functions;
mod lines;
mod record;
mod value;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_dns_zones::api::{ZoneRecords, Zones};

use lines::Lines;

const LIMIT: &str = "dns.record.max";
const LIMIT_DEFAULT: u64 = 500;

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_dns_record");

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns_record",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["dns"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.require::<dyn Zones>()
        .provide::<dyn ZoneRecords>(Arc::new(Lines))
        .function(functions::list())
        .function(functions::get())
        .function(functions::create())
        .function(functions::edit())
        .function(functions::delete())
        .contribute(
            LimitDef::new(LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}
