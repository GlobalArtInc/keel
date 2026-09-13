pub mod api;

mod authenticator;
mod entity;
mod functions;
mod ip_rule;
mod key;
mod limits;
mod password;
mod policy;
mod session;
mod su;
mod token;
mod totp;

use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::schemars::schema_for;
use keel_sdk::system::{Authenticator, PrincipalSource, SecondFactorSource, SecretCipher};

use api::{Lockout, LoginMethod, RateLimit};
use authenticator::CredentialAuthenticator;
use functions::Local;
use ip_rule::IpRules;
use password::Passwords;
use totp::challenge::Attempts;

pub use key::functions::PERMISSION_ISSUE as KEY_ISSUE;
pub use key::{KEY_PREFIX as LOGIN_KEY_PREFIX, LOGIN_PATH, MAX_TTL_SECONDS as LOGIN_KEY_MAX_TTL};
pub use limits::{GUEST_PER_MINUTE, LOCKOUT_MINUTES, LOCKOUT_THRESHOLD};
pub use password::{HASH_PREFIX, MIN_LENGTH as PASSWORD_MIN_LENGTH};
pub use session::{
    ABSOLUTE_HOURS, IDLE_MINUTES, KIND_SESSION, KIND_SU, REASON_LOGOUT, REASON_REPLACED,
    REASON_REVOKED, REASON_SU_EXIT, TOKEN_PREFIX,
};
pub use su::PERMISSION as SU;
pub use token::API_TOKEN_PREFIX;
pub use totp::challenge::{
    LIFETIME_MINUTES as CHALLENGE_MINUTES, MAX_ATTEMPTS as CHALLENGE_ATTEMPTS,
};
pub use totp::code::{
    DIGITS as TOTP_DIGITS, ISSUER as TOTP_ISSUER, RECOVERY_CODE_LEN, RECOVERY_CODES,
    STEP_SECONDS as TOTP_STEP_SECONDS, code as totp_code, decode_secret as decode_totp_secret,
    step_at as totp_step,
};

use session::functions::{PERMISSION_MANAGE as SESSION_MANAGE, PERMISSION_READ as SESSION_READ};
use token::functions::{PERMISSION_MANAGE as TOKEN_MANAGE, PERMISSION_READ as TOKEN_READ};
use totp::functions::PERMISSION_MANAGE as TOTP_MANAGE;

pub use policy::{
    SETTING_LOCKOUT_MINUTES, SETTING_LOCKOUT_THRESHOLD, SETTING_PASSWORD_MIN_LENGTH,
    SETTING_RATE_GUEST_PER_MINUTE, SETTING_SESSION_ABSOLUTE_HOURS, SETTING_SESSION_IDLE_MINUTES,
};

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/auth").table("keel_migration_auth");

pub static MODULE: ModuleDef = ModuleDef {
    id: "auth",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: Some(&MIGRATIONS),
    init,
};

pub(crate) struct State {
    methods: OnceLock<Vec<Arc<dyn LoginMethod>>>,
}

impl State {
    fn method(&self, id: &str) -> Option<&Arc<dyn LoginMethod>> {
        self.methods.get()?.iter().find(|method| method.id() == id)
    }
}

fn settings() -> [SettingDef; 6] {
    [
        SettingDef::new(SETTING_PASSWORD_MIN_LENGTH, schema_for!(u32)).default(PASSWORD_MIN_LENGTH),
        SettingDef::new(SETTING_SESSION_IDLE_MINUTES, schema_for!(u32)).default(IDLE_MINUTES),
        SettingDef::new(SETTING_SESSION_ABSOLUTE_HOURS, schema_for!(u32)).default(ABSOLUTE_HOURS),
        SettingDef::new(SETTING_LOCKOUT_THRESHOLD, schema_for!(u32)).default(LOCKOUT_THRESHOLD),
        SettingDef::new(SETTING_LOCKOUT_MINUTES, schema_for!(u32)).default(LOCKOUT_MINUTES),
        SettingDef::new(SETTING_RATE_GUEST_PER_MINUTE, schema_for!(u32)).default(GUEST_PER_MINUTE),
    ]
}

fn init(r: &mut Registrar) -> Result<()> {
    let passwords = Arc::new(Passwords::new()?);
    let state = Arc::new(State {
        methods: OnceLock::new(),
    });
    let authenticator = Arc::new(CredentialAuthenticator::default());
    let local: Arc<dyn LoginMethod> = Arc::new(Local::new(passwords.clone()));
    let contributed: Arc<dyn Authenticator> = authenticator.clone();
    let attempts = Arc::new(Attempts::default());
    let rate_limit = Arc::new(RateLimit::new());
    let lockout = Arc::new(Lockout::new());
    let ip_rules = Arc::new(IpRules::default());
    r.require::<dyn SecretCipher>()
        .require::<dyn SettingsSource>()
        .provide::<dyn SecondFactorSource>(Arc::new(totp::SecondFactor))
        .provide::<RateLimit>(rate_limit.clone())
        .provide::<Lockout>(lockout.clone())
        .guard(ip_rules.clone())
        .guard(rate_limit)
        .function(functions::password_set(passwords.clone()))
        .function(functions::login(state.clone(), lockout.clone()))
        .function(functions::logout())
        .function(functions::unlock(lockout.clone()))
        .function(token::functions::list())
        .function(token::functions::create())
        .function(token::functions::revoke_function())
        .function(totp::functions::enroll())
        .function(totp::functions::confirm())
        .function(totp::functions::disable())
        .function(totp::functions::verify(attempts, lockout))
        .function(su::su())
        .function(su::exit())
        .function(key::functions::issue())
        .function(key::functions::redeem())
        .function(ip_rule::functions::list())
        .function(ip_rule::functions::create(ip_rules.clone()))
        .function(ip_rule::functions::delete(ip_rules))
        .function(session::functions::list())
        .function(session::functions::revoke_function())
        .permission("auth.password.manage", &[Level::User, Level::Reseller])
        .permission(TOKEN_READ, &[Level::User, Level::Reseller])
        .permission(TOKEN_MANAGE, &[Level::User, Level::Reseller])
        .permission(TOTP_MANAGE, &[Level::User, Level::Reseller])
        .permission(SU, &[Level::Reseller])
        .permission(KEY_ISSUE, &[Level::Reseller])
        .permission(SESSION_READ, &[Level::User, Level::Reseller])
        .permission(SESSION_MANAGE, &[Level::User, Level::Reseller])
        .contribute(local)
        .contribute(contributed)
        .i18n("en", include_str!("../../i18n/auth/en.json"))
        .i18n("ru", include_str!("../../i18n/auth/ru.json"));
    for setting in settings() {
        r.contribute(setting);
    }
    r.start(move |ctx| async move {
        let clock = ctx.require::<dyn Clock>()?;
        let principals = ctx.require::<dyn PrincipalSource>()?;
        let settings = ctx.require::<dyn SettingsSource>()?;
        passwords.attach(principals.clone());
        authenticator.attach(clock, principals, settings);
        let methods: Vec<Arc<dyn LoginMethod>> = ctx
            .contributions::<Arc<dyn LoginMethod>>()
            .cloned()
            .collect();
        let _ = state.methods.set(methods);
        Ok(())
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FUNCTIONS: [&str; 20] = [
        "auth.password.set",
        "auth.login",
        "auth.logout",
        "auth.unlock",
        "auth.token.list",
        "auth.token.create",
        "auth.token.revoke",
        "auth.totp.enroll",
        "auth.totp.confirm",
        "auth.totp.disable",
        "auth.totp.verify",
        "auth.su",
        "auth.su.exit",
        "auth.key.issue",
        "auth.key.redeem",
        "auth.ip_rule.list",
        "auth.ip_rule.create",
        "auth.ip_rule.delete",
        "auth.session.list",
        "auth.session.revoke",
    ];

    #[test]
    fn the_module_registers_its_functions_permission_dictionaries_and_extension_points() {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        let functions: Vec<(&str, Kind, Mode, Level)> = registrations
            .functions
            .iter()
            .map(|def| (def.name(), def.kind(), def.mode(), def.access().level))
            .collect();
        assert_eq!(
            functions,
            [
                ("auth.password.set", Kind::Form, Mode::Write, Level::User),
                ("auth.login", Kind::Form, Mode::Write, Level::Guest),
                ("auth.logout", Kind::Action, Mode::Write, Level::Service),
                ("auth.unlock", Kind::Action, Mode::Write, Level::Admin),
                ("auth.token.list", Kind::List, Mode::Read, Level::User),
                ("auth.token.create", Kind::Form, Mode::Write, Level::User),
                ("auth.token.revoke", Kind::Action, Mode::Write, Level::User),
                ("auth.totp.enroll", Kind::Action, Mode::Write, Level::User),
                ("auth.totp.confirm", Kind::Form, Mode::Write, Level::User),
                ("auth.totp.disable", Kind::Form, Mode::Write, Level::User),
                ("auth.totp.verify", Kind::Form, Mode::Write, Level::Guest),
                ("auth.su", Kind::Action, Mode::Write, Level::Reseller),
                ("auth.su.exit", Kind::Action, Mode::Write, Level::Service),
                ("auth.key.issue", Kind::Form, Mode::Write, Level::Reseller),
                ("auth.key.redeem", Kind::Form, Mode::Write, Level::Guest),
                ("auth.ip_rule.list", Kind::List, Mode::Read, Level::Admin),
                ("auth.ip_rule.create", Kind::Form, Mode::Write, Level::Admin),
                (
                    "auth.ip_rule.delete",
                    Kind::Action,
                    Mode::Write,
                    Level::Admin
                ),
                ("auth.session.list", Kind::List, Mode::Read, Level::User),
                (
                    "auth.session.revoke",
                    Kind::Action,
                    Mode::Write,
                    Level::User
                ),
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, FUNCTIONS);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
        }
        let permissions: Vec<(&str, &[Level])> = registrations
            .permissions
            .iter()
            .map(|def| (def.key.as_str(), def.defaults.as_slice()))
            .collect();
        assert_eq!(
            permissions,
            [
                ("auth.password.manage", &[Level::User, Level::Reseller][..]),
                (TOKEN_READ, &[Level::User, Level::Reseller][..]),
                (TOKEN_MANAGE, &[Level::User, Level::Reseller][..]),
                (TOTP_MANAGE, &[Level::User, Level::Reseller][..]),
                (SU, &[Level::Reseller][..]),
                (KEY_ISSUE, &[Level::Reseller][..]),
                (SESSION_READ, &[Level::User, Level::Reseller][..]),
                (SESSION_MANAGE, &[Level::User, Level::Reseller][..]),
            ]
        );
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        assert_eq!(
            registrations
                .contributions
                .of::<Arc<dyn Authenticator>>()
                .count(),
            1
        );
        let methods: Vec<&str> = registrations
            .contributions
            .of::<Arc<dyn LoginMethod>>()
            .map(|method| method.id())
            .collect();
        assert_eq!(methods, [api::LOCAL_METHOD]);
        assert!(registrations.start.is_some());
        assert_eq!(registrations.guards.len(), 2);
        let settings: Vec<(&str, Option<&Value>, Level)> = registrations
            .contributions
            .of::<SettingDef>()
            .map(|def| (def.key(), def.default_value(), def.level_value()))
            .collect();
        assert_eq!(
            settings,
            [
                (SETTING_PASSWORD_MIN_LENGTH, Some(&json!(12)), Level::Admin),
                (SETTING_SESSION_IDLE_MINUTES, Some(&json!(30)), Level::Admin),
                (
                    SETTING_SESSION_ABSOLUTE_HOURS,
                    Some(&json!(12)),
                    Level::Admin
                ),
                (SETTING_LOCKOUT_THRESHOLD, Some(&json!(5)), Level::Admin),
                (SETTING_LOCKOUT_MINUTES, Some(&json!(15)), Level::Admin),
                (
                    SETTING_RATE_GUEST_PER_MINUTE,
                    Some(&json!(30)),
                    Level::Admin
                ),
            ]
        );
        assert!(
            registrations
                .provides
                .get::<dyn SecondFactorSource>()
                .is_some()
        );
        assert!(registrations.provides.get::<RateLimit>().is_some());
        assert!(registrations.provides.get::<Lockout>().is_some());
        assert_eq!(
            registrations.requires,
            [
                keel_sdk::runtime::ServiceKey::of::<dyn SecretCipher>(),
                keel_sdk::runtime::ServiceKey::of::<dyn SettingsSource>()
            ]
        );
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_auth")
        );
    }
}
