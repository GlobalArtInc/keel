pub mod api;

mod authenticator;
mod entity;
mod functions;
mod key;
mod password;
mod session;
mod su;
mod token;
mod totp;

use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::system::{Authenticator, PrincipalSource, SecondFactorSource, SecretCipher};

use api::LoginMethod;
use authenticator::CredentialAuthenticator;
use functions::Local;
use password::Passwords;
use totp::challenge::Attempts;

pub use key::functions::PERMISSION_ISSUE as KEY_ISSUE;
pub use key::{KEY_PREFIX as LOGIN_KEY_PREFIX, LOGIN_PATH, MAX_TTL_SECONDS as LOGIN_KEY_MAX_TTL};
pub use password::HASH_PREFIX;
pub use session::{
    ABSOLUTE_HOURS, IDLE_MINUTES, KIND_SESSION, KIND_SU, REASON_LOGOUT, REASON_REPLACED,
    REASON_SU_EXIT, TOKEN_PREFIX,
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

use token::functions::{PERMISSION_MANAGE as TOKEN_MANAGE, PERMISSION_READ as TOKEN_READ};
use totp::functions::PERMISSION_MANAGE as TOTP_MANAGE;

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

fn init(r: &mut Registrar) -> Result<()> {
    let passwords = Arc::new(Passwords::new()?);
    let state = Arc::new(State {
        methods: OnceLock::new(),
    });
    let authenticator = Arc::new(CredentialAuthenticator::default());
    let local: Arc<dyn LoginMethod> = Arc::new(Local::new(passwords.clone()));
    let contributed: Arc<dyn Authenticator> = authenticator.clone();
    let attempts = Arc::new(Attempts::default());
    r.require::<dyn SecretCipher>()
        .provide::<dyn SecondFactorSource>(Arc::new(totp::SecondFactor))
        .function(functions::password_set(passwords.clone()))
        .function(functions::login(state.clone()))
        .function(functions::logout())
        .function(token::functions::list())
        .function(token::functions::create())
        .function(token::functions::revoke_function())
        .function(totp::functions::enroll())
        .function(totp::functions::confirm())
        .function(totp::functions::disable())
        .function(totp::functions::verify(attempts))
        .function(su::su())
        .function(su::exit())
        .function(key::functions::issue())
        .function(key::functions::redeem())
        .permission("auth.password.manage", &[Level::User, Level::Reseller])
        .permission(TOKEN_READ, &[Level::User, Level::Reseller])
        .permission(TOKEN_MANAGE, &[Level::User, Level::Reseller])
        .permission(TOTP_MANAGE, &[Level::User, Level::Reseller])
        .permission(SU, &[Level::Reseller])
        .permission(KEY_ISSUE, &[Level::Reseller])
        .contribute(local)
        .contribute(contributed)
        .i18n("en", include_str!("../../i18n/auth/en.json"))
        .i18n("ru", include_str!("../../i18n/auth/ru.json"))
        .start(move |ctx| async move {
            let clock = ctx.require::<dyn Clock>()?;
            let principals = ctx.require::<dyn PrincipalSource>()?;
            passwords.attach(principals.clone());
            authenticator.attach(clock, principals);
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

    const FUNCTIONS: [&str; 14] = [
        "auth.password.set",
        "auth.login",
        "auth.logout",
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
        assert!(
            registrations
                .provides
                .get::<dyn SecondFactorSource>()
                .is_some()
        );
        assert_eq!(
            registrations.requires,
            [keel_sdk::runtime::ServiceKey::of::<dyn SecretCipher>()]
        );
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_auth")
        );
    }
}
