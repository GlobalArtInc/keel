mod check;
mod neighbour;
mod problems;
mod zone;

use std::sync::OnceLock;

use keel::prelude::*;
use keel::sdk::system::{Installation, PeerLinks, TaskRunner};
use keel_hosting_users::api::HostingUsers;
use keel_hosting_web::api::{AfterCommit, DnsProvider, Invoke, SiteZone, Sites, ZoneRecord};

use neighbour::{Customer, Neighbour};

pub use neighbour::{ANSWER, CHECK_TIMEOUT, CLIENT, EXISTS, ORIGIN_NAME, TEMPLATE, UNREACHABLE};
pub use problems::FAILED;
pub use zone::{CLASS, CREATE};

pub const PRODUCT: &str = "dns";
pub const UNPAIRED: &str = "dns_peer.error.unpaired";
pub const INSTANCE: &str = "dns_peer.error.instance";
const SITE_CREATE: &str = "hosting.site.create";
const NOT_STARTED: &str = "dns_peer.error.not_started";

pub static MODULE: ModuleDef = ModuleDef {
    id: "dns_peer",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["hosting_site", "hosting", "task", "notify"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let peer = Arc::new(Peer::default());
    let started = Arc::clone(&peer);
    let checking = Arc::clone(&peer);
    r.require::<dyn Sites>()
        .require::<dyn HostingUsers>()
        .require::<dyn TaskRunner>()
        .provide::<dyn DnsProvider>(Arc::clone(&peer) as Arc<dyn DnsProvider>)
        .function(zone::create(Arc::clone(&peer)))
        .before(SITE_CREATE, move |call| {
            check::zone(call, Arc::clone(&checking))
        })
        .contribute(problems::source(peer))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}

#[derive(Default)]
struct Peer {
    started: OnceLock<Started>,
}

struct Started {
    runner: Arc<dyn TaskRunner>,
    sites: Arc<dyn Sites>,
    links: Option<Arc<dyn PeerLinks>>,
    instance: Option<Id>,
}

impl Peer {
    fn attach(&self, ctx: &Context) -> Result<()> {
        let _ = self.started.set(Started {
            runner: ctx.require::<dyn TaskRunner>()?,
            sites: ctx.require::<dyn Sites>()?,
            links: ctx.require::<dyn PeerLinks>().ok(),
            instance: ctx
                .require::<Installation>()
                .ok()
                .and_then(|installation| installation.instance),
        });
        Ok(())
    }

    fn started(&self) -> Result<&Started> {
        self.started
            .get()
            .ok_or_else(|| KeelError::internal(NOT_STARTED))
    }

    fn sites(&self) -> Result<&dyn Sites> {
        Ok(self.started()?.sites.as_ref())
    }

    async fn neighbour(&self, db: &Db, zone: &str, quick: bool) -> Result<Neighbour> {
        let link = match &self.started()?.links {
            Some(links) => links.offering(PRODUCT, db).await?,
            None => None,
        };
        let link = link.ok_or_else(|| unpaired(zone))?;
        Neighbour::of(&link, quick)
    }

    fn customer(&self, account: Id) -> Result<Customer> {
        let instance = self
            .started()?
            .instance
            .ok_or_else(|| KeelError::internal(INSTANCE))?;
        Ok(Customer::of(instance, account))
    }
}

fn unpaired(zone: &str) -> KeelError {
    KeelError::unavailable(UNPAIRED).param("zone", zone)
}

#[async_trait]
impl DnsProvider for Peer {
    async fn add_zone(
        &self,
        _: &Invoke<'_>,
        after: &AfterCommit<'_>,
        site: &Id,
        _: &SiteZone,
    ) -> Result<()> {
        let runner = Arc::clone(&self.started()?.runner);
        let site = *site;
        after(Box::new(move |ctx: AfterContext| {
            Box::pin(async move { zone::spawn(runner.as_ref(), &ctx, site).await })
        }));
        Ok(())
    }

    async fn foreign(&self, _: &Invoke<'_>, _: &SiteZone) -> Result<bool> {
        Ok(false)
    }

    async fn add_records(&self, _: &Invoke<'_>, _: &SiteZone, _: &[ZoneRecord]) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use keel::sdk::system::Endpoint;
    use keel::types::ErrorCode;

    use super::*;

    #[test]
    fn the_module_provides_dns_registers_its_task_its_check_and_its_problem() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Mode, Level, bool, &str)> = registrations
            .functions
            .iter()
            .map(|def| {
                (
                    def.name(),
                    def.mode(),
                    def.access().level,
                    def.is_internal(),
                    def.class(),
                )
            })
            .collect();
        assert_eq!(functions, [(CREATE, Mode::Read, Level::User, true, CLASS)]);
        assert!(registrations.provides.get::<dyn DnsProvider>().is_some());
        let before: Vec<&str> = registrations
            .before
            .iter()
            .map(|hook| hook.function.as_str())
            .collect();
        assert_eq!(before, [SITE_CREATE]);
        assert!(registrations.within.is_empty());
        let sources: Vec<&str> = registrations
            .contributions
            .of::<keel_system::notify::api::ProblemSource>()
            .map(|source| source.id())
            .collect();
        assert_eq!(sources, [FAILED]);
        assert_eq!(registrations.contributions.of::<Endpoint>().count(), 0);
        assert!(registrations.start.is_some());
        assert_eq!(
            MODULE.requires,
            ["hosting_site", "hosting", "task", "notify"]
        );
        assert!(MODULE.migrations.is_none());
    }

    #[test]
    fn before_its_start_the_module_names_no_customer_and_reads_no_site() {
        let peer = Peer::default();
        let error = peer.customer(Id::generate()).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.key(), NOT_STARTED);
        assert_eq!(peer.sites().err().unwrap().key(), NOT_STARTED);
    }

    #[test]
    fn without_a_link_the_zone_is_unpaired_and_names_itself() {
        let error = unpaired("zone.example");
        assert_eq!(error.code(), ErrorCode::Unavailable);
        assert_eq!(error.key(), UNPAIRED);
        assert_eq!(error.params()["zone"], "zone.example");
    }
}
