mod config;
mod discover;
mod entity;
mod frames;
mod functions;
mod manifest;
mod process;
mod register;

use std::sync::{Arc, OnceLock};

use keel_sdk::prelude::*;
use keel_sdk::system::{ContextExt, Extensions, ModuleSource, PrincipalSource};

use crate::notify::api::{ProblemSource, Raise, Severity};

pub use frames::{ERROR_ANSWER, ERROR_FRAME, ERROR_INVOKE, ERROR_TOO_LARGE};
pub use functions::{DISABLE, ENABLE, ERROR_UNKNOWN, GET, LIST, LOGIN_PREFIX};
pub use manifest::FILE;
pub use process::{ERROR_FAILING, ERROR_STOPPED, PROBLEM_FAILING};
pub use register::ERROR_UNAVAILABLE;

pub const DIRECTORY: &str = "plugins";
pub const PROBLEM_REFUSED: &str = "plugin.refused";
const MENU_PATH: &str = "system/install/plugins";
const PROBLEM_SOURCE: &str = "plugin.start";

static MIGRATIONS: migrator::Migrations =
    migrator::embed!("$CARGO_MANIFEST_DIR/migrations/plugins").table("keel_migration_plugin");

pub static MODULE: ModuleDef = ModuleDef {
    id: "plugin",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &["cron", "notify"],
    migrations: Some(&MIGRATIONS),
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let shared: functions::Shared = Arc::new(OnceLock::new());
    let started = shared.clone();
    let live = Arc::new(process::Live::default());
    r.require::<dyn Clock>()
        .require::<dyn PrincipalSource>()
        .provide::<process::Live>(live.clone())
        .contribute(ConfigSection::new::<config::PluginConfig>(config::SECTION))
        .contribute(ModuleSource::new(discover::discover))
        .contribute(refusals(shared.clone()))
        .contribute(failures(live))
        .function(functions::list(shared.clone()))
        .function(functions::get(shared.clone()))
        .function(functions::enable(shared))
        .function(functions::disable())
        .menu(MenuNode::new(MENU_PATH, LIST).icon("puzzle").order(40))
        .i18n("en", include_str!("../../i18n/plugins/en.json"))
        .i18n("ru", include_str!("../../i18n/plugins/ru.json"))
        .start(move |ctx| async move {
            let extensions = ctx
                .require::<Extensions>()
                .map(|extensions| extensions.all().to_vec())
                .unwrap_or_default();
            for extension in &extensions {
                match &extension.refused {
                    Some(reason) => {
                        tracing::warn!(plugin = extension.id, reason, "the plugin is refused");
                    }
                    None => tracing::info!(
                        plugin = extension.id,
                        version = extension.version,
                        "the plugin is loaded"
                    ),
                }
            }
            let _ = started.set(functions::Started {
                data_dir: ctx.data_dir().to_path_buf(),
                extensions,
            });
            Ok(())
        });
    Ok(())
}

fn refusals(shared: functions::Shared) -> ProblemSource {
    ProblemSource::new(PROBLEM_SOURCE, move |_db| {
        let shared = shared.clone();
        async move {
            let Some(started) = shared.get() else {
                return Ok(Vec::new());
            };
            started
                .extensions
                .iter()
                .filter_map(|extension| {
                    extension.refused.as_ref().map(|reason| {
                        Raise::new(PROBLEM_REFUSED, &extension.id, Severity::Warning)
                            .params(json!({ "plugin": extension.id, "reason": reason }))
                    })
                })
                .collect()
        }
    })
}

fn failures(live: Arc<process::Live>) -> ProblemSource {
    ProblemSource::new(process::PROCESS_SOURCE, move |_db| {
        let failing = live.failing();
        async move {
            failing
                .into_iter()
                .map(|(plugin, reason)| {
                    Raise::new(PROBLEM_FAILING, &plugin, Severity::Warning)
                        .params(json!({ "plugin": plugin, "reason": reason }))
                })
                .collect()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_registers_its_functions_its_menu_its_source_and_its_problems() {
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
                (LIST, Kind::List, Mode::Read, Level::Admin),
                (GET, Kind::Get, Mode::Read, Level::Admin),
                (ENABLE, Kind::Action, Mode::Write, Level::Admin),
                (DISABLE, Kind::Action, Mode::Write, Level::Admin),
            ]
        );
        for def in &registrations.functions {
            def.descriptor().expect("the descriptor compiles");
            assert!(def.owned().is_none());
        }
        assert_eq!(registrations.menu.len(), 1);
        assert_eq!(registrations.menu[0].path(), MENU_PATH);
        assert_eq!(registrations.menu[0].func(), LIST);
        assert_eq!(registrations.contributions.of::<ModuleSource>().count(), 1);
        let sources: Vec<&str> = registrations
            .contributions
            .of::<ProblemSource>()
            .map(ProblemSource::id)
            .collect();
        assert_eq!(sources, [PROBLEM_SOURCE, process::PROCESS_SOURCE]);
        let sections: Vec<&str> = registrations
            .contributions
            .of::<ConfigSection>()
            .map(ConfigSection::key)
            .collect();
        assert_eq!(sections, [config::SECTION]);
        let required: Vec<&str> = registrations
            .requires
            .iter()
            .map(|key| key.name())
            .collect();
        assert!(required.iter().any(|name| name.ends_with("Clock")));
        assert!(
            required
                .iter()
                .any(|name| name.ends_with("PrincipalSource"))
        );
        assert!(registrations.start.is_some());
        assert_eq!(MODULE.id, "plugin");
        assert_eq!(
            MODULE.migrations.map(|migrations| migrations.ledger()),
            Some("keel_migration_plugin")
        );
    }
}
