pub mod api;
mod config;
mod frame;
mod local;

use keel_sdk::prelude::*;
use keel_sdk::system::PluginLauncher;

use api::HostExecutor;
use config::{HostConfig, SECTION};
use local::Local;

pub static MODULE: ModuleDef = ModuleDef {
    id: "host",
    version: env!("CARGO_PKG_VERSION"),
    sdk: VERSION,
    requires: &[],
    migrations: None,
    init,
};

fn init(r: &mut Registrar) -> Result<()> {
    let local = Arc::new(Local::default());
    let attached = Arc::clone(&local);
    r.provide::<dyn HostExecutor>(local.clone())
        .provide::<dyn PluginLauncher>(local)
        .contribute(ConfigSection::new::<HostConfig>(SECTION))
        .i18n("en", include_str!("../i18n/en.json"))
        .i18n("ru", include_str!("../i18n/ru.json"))
        .attach(move |ctx| async move {
            let config = ctx.config::<HostConfig>()?;
            if !attached.attach(config.target()) {
                tracing::warn!("the host module was attached twice; the first socket stands");
            }
            Ok(())
        });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_provides_the_executor_the_launcher_its_section_and_its_dictionaries_and_nothing_else()
     {
        let mut registrar = Registrar::new(MODULE.id);
        (MODULE.init)(&mut registrar).expect("init registers");
        let registrations = registrar
            .into_registrations()
            .expect("the registrations are valid");
        assert!(registrations.functions.is_empty());
        assert!(registrations.menu.is_empty());
        assert!(registrations.requires.is_empty());
        assert_eq!(registrations.provides.keys().count(), 2);
        assert!(registrations.provides.get::<dyn HostExecutor>().is_some());
        assert!(registrations.provides.get::<dyn PluginLauncher>().is_some());
        let languages: Vec<&str> = registrations
            .i18n
            .iter()
            .map(|bundle| bundle.lang.as_str())
            .collect();
        assert_eq!(languages, ["en", "ru"]);
        let sections: Vec<&ConfigSection> =
            registrations.contributions.of::<ConfigSection>().collect();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].key(), "host");
        assert!(MODULE.migrations.is_none());
        assert!(MODULE.requires.is_empty());
    }
}
