pub mod api;

mod authenticator;
mod entity;
mod functions;
mod password;
mod session;

use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::system::{Authenticator, PrincipalSource};

use api::LoginMethod;
use authenticator::SessionAuthenticator;
use functions::Local;
use password::Passwords;

pub use password::HASH_PREFIX;
pub use session::{ABSOLUTE_HOURS, IDLE_MINUTES, REASON_LOGOUT, REASON_REPLACED, TOKEN_PREFIX};

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
    let authenticator = Arc::new(SessionAuthenticator::default());
    let local: Arc<dyn LoginMethod> = Arc::new(Local::new(passwords.clone()));
    let contributed: Arc<dyn Authenticator> = authenticator.clone();
    r.function(functions::password_set(passwords.clone()))
        .function(functions::login(state.clone()))
        .function(functions::logout())
        .permission("auth.password.manage", &[Level::User, Level::Reseller])
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
            ]
        );
        let names: Vec<&str> = functions.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, ["auth.password.set", "auth.login", "auth.logout"]);
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
        }
        assert_eq!(registrations.permissions.len(), 1);
        assert_eq!(registrations.permissions[0].key, "auth.password.manage");
        assert_eq!(
            registrations.permissions[0].defaults,
            [Level::User, Level::Reseller]
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
        assert_eq!(MODULE.sdk, VERSION);
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_auth")
        );
    }
}
