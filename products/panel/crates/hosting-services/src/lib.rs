mod functions;
mod watch;

use keel::prelude::*;
use keel_host::api::HostExecutor;

use watch::Watch;

const PRODUCT: &str = "panel";

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_service",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "notify", "stats"],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let watch = Arc::new(Watch::new());
    let started = Arc::clone(&watch);
    r.require::<dyn HostExecutor>()
        .function(functions::list())
        .function(functions::start())
        .function(functions::stop())
        .function(functions::restart())
        .function(functions::reload())
        .menu(
            MenuNode::new("panel/services", functions::LIST)
                .icon("activity")
                .order(80)
                .badge(watch::DOWN),
        )
        .contribute(watch::problems(Arc::clone(&watch)))
        .contribute(watch::memory(Arc::clone(&watch)))
        .contribute(watch::uptime(Arc::clone(&watch)))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
