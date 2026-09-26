mod acme;
mod certificate;
mod config;
mod functions;
mod hooks;
mod install;
mod pem;

use keel::prelude::*;
use keel::sdk::schemars::json_schema;
use keel::sdk::system::{SecretCipher, TaskRunner};
use keel_host::api::HostExecutor;
use keel_hosting_web::api::{SiteCertificates, Sites};

use config::{CertificateConfig, SECTION};
use install::Store;

const PRODUCT: &str = "panel";
const FEATURE: &str = "panel.web";
const SITE_DELETE: &str = "hosting.site.delete";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_certificate");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_certificate",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "hosting_site", "task", "cron", "notify", "settings"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let store = Arc::new(Store::new());
    let started = Arc::clone(&store);
    let forgetting = Arc::clone(&store);
    r.require::<dyn HostExecutor>()
        .require::<dyn Sites>()
        .require::<dyn SecretCipher>()
        .require::<dyn TaskRunner>()
        .provide::<dyn SiteCertificates>(Arc::clone(&store) as Arc<dyn SiteCertificates>)
        .function(functions::list())
        .function(functions::request(Arc::clone(&store)))
        .function(functions::upload(Arc::clone(&store)))
        .function(functions::delete(Arc::clone(&store)))
        .function(functions::sites())
        .function(functions::issue(Arc::clone(&store)))
        .function(functions::renew(Arc::clone(&store)))
        .permission("hosting.certificate.read", &[Level::User, Level::Reseller])
        .permission("hosting.certificate.manage", &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/certificates", "hosting.certificate.list")
                .icon("shield-check")
                .order(30)
                .badge(hooks::EXPIRING),
        )
        .within(SITE_DELETE, move |call| {
            hooks::forget_sites(call, Arc::clone(&forgetting))
        })
        .contribute(ConfigSection::new::<CertificateConfig>(SECTION))
        .contribute(
            SettingDef::new(
                acme::DIRECTORY,
                json_schema!({ "type": "string", "format": "uri", "maxLength": 500 }),
            )
            .default(acme::PRODUCTION),
        )
        .contribute(
            SettingDef::new(
                acme::EMAIL,
                json_schema!({ "type": "string", "maxLength": 254, "pattern": "^$|^[^@\\s]+@[^@\\s]+$" }),
            )
            .default(""),
        )
        .contribute(hooks::lifecycle(Arc::clone(&store)))
        .contribute(hooks::renewal())
        .contribute(hooks::problems(Arc::clone(&store)))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
