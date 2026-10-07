use std::collections::BTreeMap;

use keel_sdk::Db;
use keel_sdk::system::{DataModule, Extension, Landscape, ModuleSource};

use super::BootError;
use super::composition::{Graph, check_id};
use super::freeze::Frozen;
use super::init::{InitError, Initialised};

impl Graph {
    pub(super) async fn load(&self, db: Db) -> Result<Frozen, BootError> {
        let baseline = self.init()?;
        let sources = baseline.sources();
        if sources.is_empty() {
            return Ok(baseline.freeze()?);
        }
        let landscape = baseline.landscape(db);
        drop(baseline);
        let mut modules: Vec<DataModule> = Vec::new();
        let mut extensions = Vec::new();
        for (module, source) in sources {
            let discovery = source
                .discover(landscape.clone())
                .await
                .map_err(|source| InitError::Discover { module, source })?;
            extensions.extend(discovery.refused);
            for found in discovery.modules {
                match self.admits(&found, &modules) {
                    Ok(()) => modules.push(found),
                    Err(reason) => extensions.push(refusal(&found, reason)),
                }
            }
        }
        if modules.is_empty() {
            return self.attempt(&modules, extensions);
        }
        if let Ok(frozen) = self.attempt(&modules, loaded(&modules, &extensions)) {
            return Ok(frozen);
        }
        let mut accepted: Vec<DataModule> = Vec::new();
        for module in modules {
            let mut trial = accepted.clone();
            trial.push(module.clone());
            match self.attempt(&trial, Vec::new()) {
                Ok(_) => accepted.push(module),
                Err(error) => extensions.push(refusal(&module, error.to_string())),
            }
        }
        let extensions = loaded(&accepted, &extensions);
        self.attempt(&accepted, extensions)
    }

    fn attempt(
        &self,
        modules: &[DataModule],
        extensions: Vec<Extension>,
    ) -> Result<Frozen, BootError> {
        Ok(self.init_with(modules, extensions)?.freeze()?)
    }

    fn admits(&self, found: &DataModule, taken: &[DataModule]) -> Result<(), String> {
        check_id(&found.id)
            .map_err(|reason| format!("the id {:?} is refused: {reason}", found.id))?;
        if self.ids().contains(&found.id.as_str()) {
            return Err(format!(
                "the id {} is a module of the composition",
                found.id
            ));
        }
        if taken.iter().any(|module| module.id == found.id) {
            return Err(format!("the id {} is discovered twice", found.id));
        }
        Ok(())
    }
}

impl Initialised {
    fn sources(&self) -> Vec<(String, ModuleSource)> {
        self.registrations
            .iter()
            .flat_map(|registrations| {
                registrations
                    .contributions
                    .of::<ModuleSource>()
                    .map(|source| (registrations.module.clone(), source.clone()))
            })
            .collect()
    }

    fn landscape(&self, db: Db) -> Landscape {
        let graph = &self.graph;
        Landscape {
            version: graph.version().to_owned(),
            data_dir: graph.data_dir().to_path_buf(),
            modules: graph
                .modules()
                .iter()
                .map(|def| (def.id.to_owned(), def.version.to_owned()))
                .collect(),
            functions: self
                .registrations
                .iter()
                .flat_map(|registrations| &registrations.functions)
                .map(|def| (def.name().to_owned(), def.access().level))
                .collect::<BTreeMap<_, _>>(),
            db,
        }
    }
}

fn refusal(module: &DataModule, reason: String) -> Extension {
    tracing::warn!(module = module.id, reason, "a data module is refused");
    Extension {
        id: module.id.clone(),
        version: module.version.clone(),
        refused: Some(reason),
    }
}

fn loaded(modules: &[DataModule], refused: &[Extension]) -> Vec<Extension> {
    modules
        .iter()
        .map(|module| Extension {
            id: module.id.clone(),
            version: module.version.clone(),
            refused: None,
        })
        .chain(refused.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use keel_sdk::prelude::*;
    use keel_sdk::system::{Discovery, Extensions};
    use keel_sdk::{KeelError, Registrar};
    use sea_orm::DatabaseConnection;

    use super::*;
    use crate::loader::testing::{get, module};
    use crate::loader::{BootError, Composition};

    fn data(id: &str, init: fn(&mut Registrar) -> Result<()>) -> DataModule {
        DataModule {
            id: id.to_owned(),
            version: "1.0.0".to_owned(),
            init: Arc::new(init),
        }
    }

    fn good(r: &mut Registrar) -> Result<()> {
        r.function(get("good.item.get"));
        Ok(())
    }

    fn homeless(r: &mut Registrar) -> Result<()> {
        r.function(get("homeless.item.get"))
            .menu(MenuNode::new("nowhere/item", "homeless.item.get"));
        Ok(())
    }

    fn foreign(r: &mut Registrar) -> Result<()> {
        r.function(get("host.item.steal"));
        Ok(())
    }

    fn sourcing_init(r: &mut Registrar) -> Result<()> {
        r.function(get("host.item.get"))
            .contribute(ModuleSource::new(|landscape: Landscape| async move {
                let seen = landscape.functions.get("host.item.get") == Some(&Level::Owner)
                    && landscape.modules.get("host").map(String::as_str) == Some("0.1.0")
                    && landscape.version == env!("CARGO_PKG_VERSION");
                let mut modules = vec![
                    data("homeless", homeless),
                    data("host", good),
                    data("Bad", good),
                    data("foreign", foreign),
                ];
                if seen {
                    modules.insert(0, data("good", good));
                }
                Ok(Discovery {
                    modules,
                    refused: vec![Extension {
                        id: "stale".to_owned(),
                        version: "0.9.0".to_owned(),
                        refused: Some("its directory is gone".to_owned()),
                    }],
                })
            }));
        Ok(())
    }

    fn failing_init(r: &mut Registrar) -> Result<()> {
        r.contribute(ModuleSource::new(|_: Landscape| async {
            Err(KeelError::internal("faulty.error.unreadable"))
        }));
        Ok(())
    }

    module!(HOST, "host", [], sourcing_init);
    module!(FAULTY, "faulty", [], failing_init);

    fn disconnected() -> Db {
        Db::from_connection(DatabaseConnection::Disconnected)
    }

    fn outcome<'a>(extensions: &'a Extensions, id: &str) -> &'a Extension {
        extensions
            .all()
            .iter()
            .find(|extension| extension.id == id)
            .unwrap_or_else(|| panic!("no outcome for {id}"))
    }

    #[tokio::test]
    async fn discovered_modules_register_through_the_registrar_and_a_faulty_one_is_refused_alone() {
        let graph = Composition::new().module(&HOST).resolve().unwrap();
        let frozen = graph.load(disconnected()).await.expect("the kernel loads");
        let registry = frozen.registry();
        assert_eq!(
            registry.function("good.item.get").map(|f| f.module()),
            Some("good")
        );
        assert!(registry.function("foreign.item.get").is_none());
        assert!(registry.function("host.item.steal").is_none());
        assert!(registry.function("homeless.item.get").is_none());
        assert!(registry.function("host.item.get").is_some());
        let modules: Vec<(&str, &str)> = registry
            .modules()
            .iter()
            .map(|info| (info.id.as_str(), info.version.as_str()))
            .collect();
        assert_eq!(
            modules,
            [
                ("keel", env!("CARGO_PKG_VERSION")),
                ("host", "0.1.0"),
                ("good", "1.0.0")
            ]
        );
        let extensions = registry
            .services()
            .get::<Extensions>()
            .expect("the kernel provides the outcomes");
        assert_eq!(outcome(&extensions, "good").refused, None);
        let refused = |id: &str| {
            outcome(&extensions, id)
                .refused
                .clone()
                .unwrap_or_else(|| panic!("{id} is loaded"))
        };
        assert!(
            refused("homeless").contains("nowhere"),
            "{}",
            refused("homeless")
        );
        assert_eq!(
            refused("host"),
            "the id host is a module of the composition"
        );
        assert!(
            refused("Bad").starts_with("the id \"Bad\" is refused"),
            "{}",
            refused("Bad")
        );
        assert!(
            refused("foreign").contains("host.item.steal"),
            "{}",
            refused("foreign")
        );
        assert_eq!(refused("stale"), "its directory is gone");
        assert_eq!(extensions.all().len(), 6);
    }

    #[tokio::test]
    async fn a_failing_source_fails_the_boot_naming_its_module() {
        let graph = Composition::new().module(&FAULTY).resolve().unwrap();
        let error = graph
            .load(disconnected())
            .await
            .expect_err("the source fails");
        assert!(
            matches!(&error, BootError::Init(InitError::Discover { module, .. }) if module == "faulty"),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            "module faulty: discovering its data modules failed: internal: faulty.error.unreadable"
        );
    }

    #[tokio::test]
    async fn without_a_source_the_load_is_the_plain_init_and_freeze() {
        let graph = Composition::new().resolve().unwrap();
        let frozen = graph.load(disconnected()).await.expect("the kernel loads");
        let extensions = frozen
            .registry()
            .services()
            .get::<Extensions>()
            .expect("the outcomes are provided even when empty");
        assert!(extensions.all().is_empty());
        assert_eq!(frozen.registry().modules().len(), 1);
    }
}
