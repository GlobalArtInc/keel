mod authenticator;
mod check;
mod codes;
mod delivery;
mod entity;
mod functions;
mod identity;
mod inbox;
mod links;
mod on_behalf;
mod pairing;
mod proof;
mod pull;
mod remote;
mod rotation;

use std::sync::Arc;

use keel_sdk::InstanceInfoSource;
use keel_sdk::prelude::*;
use keel_sdk::system::{
    AccountSource, Authenticator, Caller, ContextExt, Degradation, EventRelay, Features,
    Installation, OnBehalf, OutboxSubscriber, PeerLinks, PeerReach, PrincipalSource, SecretCipher,
};
use keel_sdk::types::Scope;

use authenticator::PeerAuthenticator;
use delivery::Delivery;
use links::Links;

use crate::auth::api::RateLimit;

pub use authenticator::ERROR_PRINCIPAL;
pub use check::{JOB_ID, PROBLEM_INSTANCE_KEY, PROBLEM_PROOF, PROBLEM_PROTOCOL, SCHEDULE, SOURCE};
pub use codes::{CODE_PREFIX, CODE_SYMBOLS};
pub use delivery::{
    ERROR_EVENT, ERROR_FEED, ERROR_QUERY, ERROR_SIGNATURE, FEED, FEED_BATCH, FEED_LIMIT,
    FEED_LIMIT_MAX, FEED_SCAN, HOOK, MAX_TYPES, YEARS,
};
pub use functions::{
    DELETE, ERROR_STORED, EVENT_CREATED, EVENT_DELETED, EVENT_ROTATED, GET, LIST, STATUS_ACTIVE,
};
pub use identity::{MASKS as IDENTITY_MASKS, UPSERTS};
pub use inbox::{
    BATCH as INBOX_BATCH, MAX_ATTEMPTS as INBOX_MAX_ATTEMPTS, PENDING_MAX as INBOX_PENDING_MAX,
    POLL as INBOX_POLL, RETENTION_DAYS as INBOX_RETENTION_DAYS,
};
pub use on_behalf::{
    ERROR_CUSTOMER, ERROR_HEADER, ERROR_ORIGIN, MAX_EXTERNAL_ID, is_external_id, names_peer,
};
pub use pairing::{
    CREATE, ERROR_CODE, ERROR_INSTANCE_KEY, ERROR_LEVEL, ERROR_MASK, ERROR_PAIRED, ERROR_PROTOCOL,
    ERROR_SCOPE, ERROR_SELF, ERROR_TOKEN, LINK, LOGIN_PREFIX, MAX_MASKS, PAIR, PAIR_CODE, ROLE,
    TTL_DEFAULT, TTL_MAX, TTL_MIN, WEBHOOK_SECRET_PREFIX, compatible,
};
pub use proof::CHALLENGE_BYTES;
pub use pull::{LIMIT as PULL_LIMIT, PAUSE as PULL_PAUSE, ROUNDS as PULL_ROUNDS};
pub use remote::{
    ERROR_ANSWER, ERROR_HANDSHAKE, ERROR_UNREACHABLE, ERROR_URL, FEED_PATH, HOOK_PATH,
};
pub use rotation::{ERROR_NO_URL, ERROR_PROOF, REKEY, RENEW, ROTATE};

pub const FEATURE: &str = "peers";
pub const MENU_PATH: &str = "system/peers";
pub const MENU_ICON: &str = "network";
pub const FUNCTIONS: [&str; 10] = [
    LIST, GET, PAIR_CODE, PAIR, CREATE, LINK, ROTATE, RENEW, REKEY, DELETE,
];
const LICENSE: &str = "keel.error.license";
const READ_ONLY: &str = "read_only";
const TOKEN_READ_ONLY: &str = "keel.error.token_read_only";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/peers").table("keel_migration_peer");

pub static MODULE: ModuleDef = ModuleDef {
    id: "peer",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["auth", "cron", "notify", "webhook"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn continuation(
    principal: &Principal,
    features: Option<&dyn Features>,
    function: &str,
) -> Result<Principal> {
    if !principal.scope.admits_write() {
        return Err(KeelError::forbidden(TOKEN_READ_ONLY).param("function", function));
    }
    match features.and_then(|features| features.degradation(MODULE.id)) {
        None => {}
        Some(Degradation::ReadOnly) => {
            return Err(KeelError::license(LICENSE)
                .param("function", function)
                .param("state", READ_ONLY));
        }
        Some(Degradation::Unlisted { product }) => {
            return Err(KeelError::license(LICENSE)
                .param("function", function)
                .param("product", product));
        }
    }
    let mut principal = principal.clone();
    principal.scope = Scope::Full;
    Ok(principal)
}

fn init(r: &mut Registrar) -> Result<()> {
    let authenticator = Arc::new(PeerAuthenticator::default());
    let contributed: Arc<dyn Authenticator> = authenticator.clone();
    let delegate: Arc<dyn OnBehalf> = authenticator.clone();
    let reach: Arc<dyn PeerReach> = authenticator.clone();
    let links = Arc::new(Links::default());
    let offered: Arc<dyn PeerLinks> = links.clone();
    let delivery = Arc::new(Delivery::default());
    let refusals = Arc::new(RateLimit::new());
    let [hook, feed] = delivery::endpoints(&delivery);
    r.require::<dyn Caller>()
        .require::<dyn PrincipalSource>()
        .require::<dyn AccountSource>()
        .require::<dyn SecretCipher>()
        .require::<dyn OutboxSubscriber>()
        .require::<dyn EventRelay>()
        .require::<dyn SettingsSource>()
        .contribute(InstanceInfoSource::new().feature(FEATURE))
        .contribute(hook)
        .contribute(feed)
        .contribute(contributed)
        .contribute(delegate)
        .provide(reach)
        .provide(offered)
        .contribute(check::job())
        .within(UPSERTS[0], identity::bound)
        .within(UPSERTS[1], identity::bound)
        .function(functions::list())
        .function(functions::get())
        .function(pairing::pair_code())
        .function(pairing::pair())
        .function(pairing::create())
        .function(pairing::link())
        .function(rotation::rotate())
        .function(rotation::renew())
        .function(rotation::rekey())
        .function(functions::delete())
        .menu(MenuNode::new(MENU_PATH, LIST).icon(MENU_ICON).order(55))
        .i18n("en", include_str!("../../i18n/peers/en.json"))
        .i18n("ru", include_str!("../../i18n/peers/ru.json"))
        .start(move |ctx| async move {
            authenticator.attach(
                ctx.require::<dyn Clock>()?,
                ctx.require::<dyn PrincipalSource>()?,
                ctx.require::<dyn AccountSource>()?,
            );
            links.attach(ctx.require::<dyn SecretCipher>()?);
            delivery.attach(delivery::Seams {
                db: ctx.db(),
                clock: ctx.require::<dyn Clock>()?,
                cipher: ctx.require::<dyn SecretCipher>()?,
                subscriber: ctx.require::<dyn OutboxSubscriber>()?,
                relay: ctx.require::<dyn EventRelay>()?,
                caller: ctx.require::<dyn Caller>()?,
                principals: ctx.require::<dyn PrincipalSource>()?,
                accounts: ctx.require::<dyn AccountSource>()?,
                reach: ctx.require::<dyn PeerReach>()?,
                settings: ctx.require::<dyn SettingsSource>()?,
                refusals,
                instance: ctx
                    .require::<Installation>()
                    .ok()
                    .and_then(|installation| installation.instance),
            });
            ctx.spawn(inbox::run(delivery.clone()));
            ctx.spawn(pull::run(delivery));
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use keel_sdk::types::{ErrorCode, PeerScope, Via};

    use super::*;
    use crate::cron::api::CronJob;

    #[test]
    fn the_module_registers_its_functions_its_menu_its_job_and_its_authenticator() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode, Level, bool)> = registrations
            .functions
            .iter()
            .map(|def| {
                (
                    def.name(),
                    def.kind(),
                    def.mode(),
                    def.access().level,
                    def.is_internal(),
                )
            })
            .collect();
        assert_eq!(
            functions,
            [
                (LIST, Kind::List, Mode::Read, Level::Admin, false),
                (GET, Kind::Get, Mode::Read, Level::Admin, false),
                (PAIR_CODE, Kind::Form, Mode::Write, Level::Admin, false),
                (PAIR, Kind::Action, Mode::Write, Level::Guest, false),
                (CREATE, Kind::Form, Mode::Read, Level::Admin, false),
                (LINK, Kind::Action, Mode::Write, Level::Admin, true),
                (ROTATE, Kind::Action, Mode::Read, Level::Admin, false),
                (RENEW, Kind::Action, Mode::Write, Level::Guest, false),
                (REKEY, Kind::Action, Mode::Write, Level::Admin, true),
                (DELETE, Kind::Action, Mode::Write, Level::Admin, false),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            assert!(def.owned().is_none(), "{}", def.name());
            def.descriptor().expect("the descriptor compiles");
        }
        let sources: Vec<&InstanceInfoSource> = registrations
            .contributions
            .of::<InstanceInfoSource>()
            .collect();
        assert_eq!(sources, [&InstanceInfoSource::new().feature(FEATURE)]);
        assert_eq!(
            registrations
                .contributions
                .of::<Arc<dyn Authenticator>>()
                .count(),
            1
        );
        assert_eq!(
            registrations
                .contributions
                .of::<Arc<dyn OnBehalf>>()
                .count(),
            1
        );
        assert!(registrations.provides.get::<dyn PeerReach>().is_some());
        assert!(registrations.provides.get::<dyn PeerLinks>().is_some());
        let endpoints: Vec<(&str, &str, &str)> = registrations
            .contributions
            .of::<keel_sdk::system::Endpoint>()
            .map(|endpoint| {
                (
                    endpoint.module(),
                    endpoint.method().as_str(),
                    endpoint.name(),
                )
            })
            .collect();
        assert_eq!(endpoints, [("peer", "POST", HOOK), ("peer", "GET", FEED)]);
        let jobs: Vec<(&str, &str)> = registrations
            .contributions
            .of::<CronJob>()
            .map(|job| (job.id(), job.schedule()))
            .collect();
        assert_eq!(jobs, [(JOB_ID, SCHEDULE)]);
        let hooked: Vec<&str> = registrations
            .within
            .iter()
            .map(|hook| hook.function.as_str())
            .collect();
        assert_eq!(hooked, UPSERTS);
        assert!(registrations.before.is_empty());
        assert!(registrations.permissions.is_empty());
        let menu: Vec<(&str, &str, Option<&str>)> = registrations
            .menu
            .iter()
            .map(|node| (node.path(), node.func(), node.icon_name()))
            .collect();
        assert_eq!(menu, [(MENU_PATH, LIST, Some(MENU_ICON))]);
        assert!(registrations.start.is_some());
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(MODULE.id, "peer");
        assert_eq!(MODULE.requires, ["auth", "cron", "notify", "webhook"]);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_peer")
        );
    }

    struct Degraded(Option<Degradation>);

    impl Features for Degraded {
        fn enabled(&self) -> Vec<String> {
            Vec::new()
        }

        fn allows(&self, _: &str) -> bool {
            true
        }

        fn degradation(&self, module: &str) -> Option<Degradation> {
            assert_eq!(module, MODULE.id);
            self.0.clone()
        }
    }

    fn admin(scope: Scope) -> Principal {
        Principal {
            id: Id::generate(),
            login: "root".into(),
            level: Level::Admin,
            account_id: Id::ROOT_ACCOUNT,
            role: "owner".into(),
            actor: None,
            scope,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
            grants: Default::default(),
        }
    }

    #[test]
    fn a_continuation_writes_as_the_caller_with_a_full_scope_unless_the_token_or_the_lease_is_read_only()
     {
        let scoped = admin(Scope::Functions(vec![CREATE.into()]));
        let continued = continuation(&scoped, None, CREATE).unwrap();
        assert_eq!(continued.scope, Scope::Full);
        assert_eq!(continued.id, scoped.id);
        assert_eq!(continued.level, Level::Admin);
        let peer = admin(Scope::Peer(PeerScope {
            functions: vec!["peer.*".into()],
            level: Level::Admin,
            on_behalf: false,
        }));
        assert_eq!(
            continuation(&peer, Some(&Degraded(None)), CREATE)
                .unwrap()
                .scope,
            Scope::Full
        );
        let read_only = continuation(&admin(Scope::ReadOnly), None, CREATE).unwrap_err();
        assert_eq!(read_only.code(), ErrorCode::Forbidden);
        assert_eq!(read_only.key(), TOKEN_READ_ONLY);
        assert_eq!(read_only.params()["function"], CREATE);
        let lease = continuation(
            &admin(Scope::Full),
            Some(&Degraded(Some(Degradation::ReadOnly))),
            ROTATE,
        )
        .unwrap_err();
        assert_eq!(lease.code(), ErrorCode::License);
        assert_eq!(lease.params()["state"], READ_ONLY);
        assert_eq!(lease.params()["function"], ROTATE);
        let unlisted = continuation(
            &admin(Scope::Full),
            Some(&Degraded(Some(Degradation::Unlisted {
                product: "dns".into(),
            }))),
            ROTATE,
        )
        .unwrap_err();
        assert_eq!(unlisted.code(), ErrorCode::License);
        assert_eq!(unlisted.params()["product"], "dns");
    }
}
