pub mod api;
mod config;
mod domain;
mod domains;
mod host;
mod lifecycle;
mod limit;
mod lookup;
mod mailbox;
mod mailboxes;
mod names;

use keel::prelude::*;
use keel::sdk::limit::Unit;
use keel_host::api::HostExecutor;
use keel_hosting_web::api::DnsProvider;

use config::{MailConfig, SECTION};
use host::{DOVECOT_CHECK, MAP_TEMPLATE, Mail, PASSWD_TEMPLATE, POSTFIX_CHECK};
use lookup::Lookup;

const PRODUCT: &str = "panel";
const FEATURE: &str = "panel.mail";
const READ: &str = "hosting.mail.read";
const MANAGE: &str = "hosting.mail.manage";
const DOMAIN_LIMIT: &str = "hosting.mail.domain.max";
const MAILBOX_LIMIT: &str = "hosting.mail.mailbox.max";
const DOMAIN_LIMIT_DEFAULT: u64 = 10;
const MAILBOX_LIMIT_DEFAULT: u64 = 50;
const BACKUP: &str = "hosting_mail.maildirs";
const STATE_PENDING: &str = "pending";
const STATE_ACTIVE: &str = "active";
const STATE_ERROR: &str = "error";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations").table("keel_migration_hosting_mail");

pub static MODULE: ModuleDef = ModuleDef {
    id: "hosting_mail",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["host", "hosting_site"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let mail = Arc::new(Mail::default());
    let started = Arc::clone(&mail);
    let archived = Arc::clone(&mail);
    r.require::<dyn HostExecutor>()
        .require::<dyn DnsProvider>()
        .provide::<dyn api::Mailboxes>(Arc::new(Lookup))
        .function(domains::list())
        .function(domains::create(Arc::clone(&mail)))
        .function(domains::delete(Arc::clone(&mail)))
        .function(domains::options())
        .function(mailboxes::list())
        .function(mailboxes::get())
        .function(mailboxes::create(Arc::clone(&mail)))
        .function(mailboxes::edit(Arc::clone(&mail)))
        .function(mailboxes::delete(Arc::clone(&mail)))
        .permission(READ, &[Level::User, Level::Reseller])
        .permission(MANAGE, &[Level::User, Level::Reseller])
        .menu(
            MenuNode::new("panel/mail-domains", "hosting.mail.domain.list")
                .icon("mail")
                .order(50),
        )
        .menu(
            MenuNode::new("panel/mailboxes", "hosting.mail.mailbox.list")
                .icon("inbox")
                .order(51),
        )
        .within(domains::CREATE, limit::domains)
        .within(mailboxes::CREATE, limit::mailboxes)
        .contribute(
            LimitDef::new(DOMAIN_LIMIT, Unit::Count)
                .default(DOMAIN_LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(
            LimitDef::new(MAILBOX_LIMIT, Unit::Count)
                .default(MAILBOX_LIMIT_DEFAULT)
                .unlimited(true),
        )
        .contribute(ConfigSection::new::<MailConfig>(SECTION))
        .contribute(lifecycle::contribution(mail))
        .contribute(
            ConfigTemplate::new(
                MAP_TEMPLATE,
                include_str!("../templates/panel/postfix-map.j2"),
            )
            .validate(POSTFIX_CHECK),
        )
        .contribute(
            ConfigTemplate::new(
                PASSWD_TEMPLATE,
                include_str!("../templates/panel/dovecot-passwd.j2"),
            )
            .validate(DOVECOT_CHECK),
        )
        .contribute(BackupSource::new(BACKUP, move |dir: BackupDir| {
            let archived = Arc::clone(&archived);
            async move { archived.archive(&dir).await }
        }))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .start(move |ctx| async move { started.attach(&ctx) });
    Ok(())
}
