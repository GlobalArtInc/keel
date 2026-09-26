pub mod api;
mod config;
mod functions;
mod host;
mod lifecycle;
mod limit;
mod names;
mod overrides;
mod site;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel::sdk::tokio::sync::Mutex;
use keel_host::api::HostExecutor;
use keel_hosting_users::api::HostingUsers;

use host::{NGINX_CHECK, POOL_TEMPLATE, Renderer, SERVER_TEMPLATE};

const PRODUCT: &str = "panel";
const FEATURE: &str = "panel.web";
const LIMIT: &str = "hosting.site.max";
const LIMIT_DEFAULT: u64 = 10;

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_site");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_site",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "hosting"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let turn = Arc::new(Mutex::new(()));
    let renderer = Arc::new(Renderer::new(Arc::clone(&turn)));
    let started = Arc::clone(&renderer);
    r.require::<dyn HostExecutor>()
        .require::<dyn HostingUsers>()
        .require::<dyn api::DnsProvider>()
        .provide::<dyn api::Sites>(renderer)
        .function(functions::list())
        .function(functions::get())
        .function(functions::create(Arc::clone(&turn)))
        .function(functions::edit(Arc::clone(&turn)))
        .function(functions::delete(Arc::clone(&turn)))
        .function(functions::php())
        .function(config::get(Arc::clone(&turn)))
        .function(config::set(Arc::clone(&turn)))
        .before(config::SET, {
            let turn = Arc::clone(&turn);
            move |call| config::trial(call, Arc::clone(&turn))
        })
        .view(
            config::VIEW,
            ViewSpec::new(Level::Admin)
                .menu(
                    MenuNode::new("panel/config", config::VIEW)
                        .label("hosting.site.config-editor.title")
                        .icon("file-code")
                        .order(25),
                )
                .needs([FEATURE]),
        )
        .permission("hosting.site.read", &[Level::User, Level::Reseller])
        .permission("hosting.site.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/sites", "hosting.site.list")
                .icon("globe")
                .order(20),
        )
        .within("hosting.site.create", limit::enforce)
        .within("hosting.user.delete", limit::refuse_busy_users)
        .contribute(
            LimitDef::new(LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(lifecycle::contribution(turn))
        .contribute(
            ConfigTemplate::new(
                SERVER_TEMPLATE,
                include_str!("../templates/panel/nginx-site.conf.j2"),
            )
            .validate(NGINX_CHECK),
        )
        .contribute(ConfigTemplate::new(
            POOL_TEMPLATE,
            include_str!("../templates/panel/php-fpm-pool.conf.j2"),
        ))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
