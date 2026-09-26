use keel::prelude::*;
use keel_hosting_web::api::{DnsProvider, Invoke, SiteZone, ZoneRecord};

const UNPAIRED: &str = "dns_peer.error.unpaired";

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns_peer",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["hosting_site"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    r.provide::<dyn DnsProvider>(Arc::new(Peer))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"));
    Ok(())
}

struct Peer;

#[async_trait]
impl DnsProvider for Peer {
    async fn add_zone(&self, _: &Invoke<'_>, zone: &SiteZone) -> Result<()> {
        Err(KeelError::unavailable(UNPAIRED).param("zone", zone.name.as_str()))
    }

    async fn add_records(&self, _: &Invoke<'_>, _: &SiteZone, _: &[ZoneRecord]) -> Result<bool> {
        Ok(false)
    }
}
