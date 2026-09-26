mod account;
mod functions;
mod host;
mod lifecycle;
mod names;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_host::api::HostExecutor;
use keel_hosting_web::api::Sites;

use host::{Ftp, PASSWD_TEMPLATE, USER_TEMPLATE, VSFTPD_CHECK};

const PRODUCT: &str = "panel";
const FEATURE: &str = "panel.ftp";
const READ: &str = "hosting.ftp.read";
const MANAGE: &str = "hosting.ftp.manage";
const LIMIT: &str = "hosting.ftp.max";
const LIMIT_DEFAULT: u64 = 10;
const SITE_EDIT: &str = "hosting.site.edit";
const SITE_DELETE: &str = "hosting.site.delete";
const STATE_PENDING: &str = "pending";
const STATE_ACTIVE: &str = "active";
const STATE_ERROR: &str = "error";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_ftp");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_ftp",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "hosting_site"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let ftp = Arc::new(Ftp::default());
    let started = Arc::clone(&ftp);
    let following = Arc::clone(&ftp);
    let forgetting = Arc::clone(&ftp);
    r.require::<dyn HostExecutor>()
        .require::<dyn Sites>()
        .function(functions::list())
        .function(functions::get())
        .function(functions::create(Arc::clone(&ftp)))
        .function(functions::edit(Arc::clone(&ftp)))
        .function(functions::delete(Arc::clone(&ftp)))
        .function(functions::sites())
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/ftp", "hosting.ftp.list")
                .icon("upload")
                .order(60),
        )
        .within(SITE_EDIT, move |call| {
            lifecycle::follow_site(call, Arc::clone(&following))
        })
        .within(SITE_DELETE, move |call| {
            lifecycle::forget_sites(call, Arc::clone(&forgetting))
        })
        .contribute(
            LimitDef::new(LIMIT, Unit::Count)
                .default(LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(lifecycle::contribution(ftp))
        .contribute(
            ConfigTemplate::new(
                USER_TEMPLATE,
                include_str!("../templates/panel/vsftpd-user.conf.j2"),
            )
            .validate(VSFTPD_CHECK),
        )
        .contribute(
            ConfigTemplate::new(
                PASSWD_TEMPLATE,
                include_str!("../templates/panel/vsftpd-passwd.j2"),
            )
            .validate(VSFTPD_CHECK),
        )
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
