mod bind;
mod config;
mod functions;
mod hooks;
mod lifecycle;
mod problems;
mod provider;
mod zone;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_host::api::HostExecutor;

use bind::BindProvider;
use config::{DnsConfig, SECTION};
use provider::DnsProvider;

const PRODUCT: &str = "dns";
const DROP_IN_PREFIX: &str = "keel-dns-";
const LIMIT: &str = "dns.zones";
const LIMIT_DEFAULT: u64 = 50;
const FAILED: &str = "dns.zone.failed";
const EXISTS: &str = "dns.zone.error.exists";
const BACKUP: &str = "dns.zonefiles";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_dns");

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "notify"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let bind = Arc::new(BindProvider::default());
    let started = Arc::clone(&bind);
    let backed_up = Arc::clone(&bind);
    r.require::<dyn HostExecutor>()
        .provide::<dyn DnsProvider>(bind)
        .function(functions::list())
        .function(functions::get())
        .function(functions::create())
        .function(functions::edit())
        .function(functions::delete())
        .permission("dns.zone.read", &[Level::User, Level::Reseller])
        .permission("dns.zone.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("dns/zones", "dns.zone.list")
                .icon("globe")
                .order(10),
        )
        .within("dns.zone.create", hooks::enforce_limit)
        .contribute(ConfigSection::new::<DnsConfig>(SECTION))
        .contribute(
            LimitDef::new(LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(lifecycle::contribution())
        .contribute(problems::source())
        .contribute(
            ConfigTemplate::new(
                bind::ZONE_TEMPLATE,
                include_str!("../templates/bind/zone.j2"),
            )
            .validate(bind::CHECK),
        )
        .contribute(
            ConfigTemplate::new(
                bind::INCLUDE_TEMPLATE,
                include_str!("../templates/bind/zone.conf.j2"),
            )
            .validate(bind::CHECK),
        )
        .contribute(
            ConfigTemplate::new(
                bind::INDEX_TEMPLATE,
                include_str!("../templates/bind/zones.conf.j2"),
            )
            .validate(bind::CHECK),
        )
        .contribute(BackupSource::new(BACKUP, move |dir: BackupDir| {
            let bind = Arc::clone(&backed_up);
            async move { bind.copy_zone_files(&dir).await }
        }))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move {
            let host = ctx.require::<dyn HostExecutor>()?;
            let config = ctx.config::<DnsConfig>()?;
            started.attach(host, config);
            Ok(())
        });
    Ok(())
}
