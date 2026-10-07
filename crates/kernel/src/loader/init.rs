use std::fmt;

use keel_sdk::runtime::{Contributions, Derived, RegistrationError, Registrations};
use keel_sdk::system::{DataModule, Extension};
use keel_sdk::{KeelError, Registrar};

use super::composition::Graph;

pub struct Initialised {
    pub(super) graph: Graph,
    pub(super) registrations: Vec<Registrations>,
    pub(super) extensions: Vec<Extension>,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("module {module}: init failed: {source}")]
    Init {
        module: String,
        #[source]
        source: KeelError,
    },
    #[error("module {module}: discovering its data modules failed: {source}")]
    Discover {
        module: String,
        #[source]
        source: KeelError,
    },
    #[error("module {module}: deriving from the contributions failed: {source}")]
    Derive {
        module: String,
        #[source]
        source: KeelError,
    },
    #[error("module {module}: init registered invalid entries:{}", list(.errors))]
    Registration {
        module: String,
        errors: Vec<RegistrationError>,
    },
}

fn list(errors: &[RegistrationError]) -> String {
    errors
        .iter()
        .map(|error| format!("\n  - {error}"))
        .collect()
}

impl Graph {
    pub fn init(&self) -> Result<Initialised, InitError> {
        self.init_with(&[], Vec::new())
    }

    pub(super) fn init_with(
        &self,
        data: &[DataModule],
        extensions: Vec<Extension>,
    ) -> Result<Initialised, InitError> {
        let mut registrars = Vec::with_capacity(self.modules().len() + data.len());
        for def in self.modules() {
            let mut registrar = Registrar::new(def.id);
            (def.init)(&mut registrar).map_err(|source| InitError::Init {
                module: def.id.to_owned(),
                source,
            })?;
            registrars.push(registrar);
        }
        for module in data {
            let mut registrar = Registrar::new(&module.id);
            (module.init)(&mut registrar).map_err(|source| InitError::Init {
                module: module.id.clone(),
                source,
            })?;
            registrars.push(registrar);
        }
        for (index, derived) in derive(&mut registrars) {
            let registrar = &mut registrars[index];
            derived(registrar).map_err(|source| InitError::Derive {
                module: registrar.module().to_owned(),
                source,
            })?;
        }
        let mut registrations = Vec::with_capacity(registrars.len());
        for registrar in registrars {
            let module = registrar.module().to_owned();
            let registered =
                registrar
                    .into_registrations()
                    .map_err(|errors| InitError::Registration {
                        module: module.clone(),
                        errors,
                    })?;
            tracing::debug!(
                module,
                functions = registered.functions.len(),
                "module initialised"
            );
            registrations.push(registered);
        }
        Ok(Initialised {
            graph: self.clone(),
            registrations,
            extensions,
        })
    }
}

fn derive(registrars: &mut [Registrar]) -> Vec<(usize, Derived)> {
    let hooks: Vec<_> = registrars
        .iter_mut()
        .enumerate()
        .filter_map(|(index, registrar)| registrar.take_derive().map(|hook| (index, hook)))
        .collect();
    let bags: Vec<&Contributions> = registrars.iter().map(Registrar::contributions).collect();
    hooks
        .into_iter()
        .map(|(index, hook)| (index, hook(&bags)))
        .collect()
}

impl Initialised {
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
}

impl fmt::Debug for Initialised {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Initialised")
            .field("graph", &self.graph)
            .field("registrations", &self.registrations)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use keel_sdk::prelude::*;
    use keel_sdk::{InstanceInfoSource, Registrar};

    use super::*;
    use crate::loader::Composition;
    use crate::loader::testing::{Nothing, module};

    #[derive(Clone)]
    struct Offer(&'static str);

    fn action(name: &str) -> FunctionDef {
        function::<Nothing, Nothing>(name)
            .kind(Kind::Action)
            .level(Level::Admin)
            .unowned()
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
    }

    fn deriving_init(r: &mut Registrar) -> Result<()> {
        r.contribute(Offer("own")).derive(|offers: Vec<Offer>, r| {
            let mut source = InstanceInfoSource::new();
            for offer in offers {
                r.function(action(&format!("broker.{}.open", offer.0)));
                source = source.service_kind(offer.0);
            }
            r.contribute(source);
            Ok(())
        });
        Ok(())
    }

    fn sky_init(r: &mut Registrar) -> Result<()> {
        r.contribute(Offer("sky.zones"))
            .contribute(Offer("sky.boxes"));
        Ok(())
    }

    fn sea_init(r: &mut Registrar) -> Result<()> {
        r.contribute(Offer("sea.nets"));
        Ok(())
    }

    fn twin_init(r: &mut Registrar) -> Result<()> {
        r.contribute(Offer("own"));
        Ok(())
    }

    fn failing_init(r: &mut Registrar) -> Result<()> {
        r.derive(|offers: Vec<Offer>, _| {
            Err(KeelError::internal("faulty.error.offers").param("count", offers.len()))
        });
        Ok(())
    }

    module!(BROKER, "broker", [], deriving_init);
    module!(SKY, "sky", ["broker"], sky_init);
    module!(SEA, "sea", [], sea_init);
    module!(TWIN, "twin", [], twin_init);
    module!(FAULTY, "faulty", [], failing_init);

    fn initialised(modules: &[&'static ModuleDef]) -> Result<Initialised, InitError> {
        let mut composition = Composition::new();
        for module in modules {
            composition = composition.module(module);
        }
        composition
            .resolve()
            .expect("the composition resolves")
            .init()
    }

    fn of<'a>(initialised: &'a Initialised, module: &str) -> &'a Registrations {
        initialised
            .registrations
            .iter()
            .find(|registrations| registrations.module == module)
            .expect("the module is initialised")
    }

    #[test]
    fn a_derive_hook_sees_the_contributions_of_every_module_even_those_initialised_after_it() {
        let initialised = initialised(&[&SKY, &BROKER, &SEA]).expect("init registers");
        assert_eq!(initialised.graph().ids(), ["keel", "broker", "sea", "sky"]);
        let broker = of(&initialised, "broker");
        let names: Vec<&str> = broker.functions.iter().map(FunctionDef::name).collect();
        assert_eq!(
            names,
            [
                "broker.own.open",
                "broker.sea.nets.open",
                "broker.sky.zones.open",
                "broker.sky.boxes.open",
            ]
        );
        assert!(broker.functions.iter().all(|def| def.module() == "broker"));
        let kinds: Vec<&String> = broker
            .contributions
            .of::<InstanceInfoSource>()
            .flat_map(|source| source.service_kinds())
            .collect();
        assert_eq!(kinds, ["own", "sea.nets", "sky.zones", "sky.boxes"]);
        assert!(of(&initialised, "sky").functions.is_empty());
        let frozen = initialised.freeze().expect("the derived functions freeze");
        assert!(
            frozen
                .registry()
                .function("broker.sky.zones.open")
                .is_some()
        );
    }

    #[test]
    fn a_derived_registration_is_checked_like_one_of_init() {
        let error = initialised(&[&BROKER, &TWIN]).expect_err("the name is taken twice");
        assert_eq!(
            error.to_string(),
            "module broker: init registered invalid entries:\n  - module broker: function \"broker.own.open\" is registered twice"
        );
    }

    #[test]
    fn a_failing_derive_hook_names_its_module() {
        let error = initialised(&[&FAULTY, &SEA]).expect_err("the hook fails");
        assert!(matches!(&error, InitError::Derive { module, .. } if module == "faulty"));
        assert!(
            error
                .to_string()
                .starts_with("module faulty: deriving from the contributions failed:"),
            "{error}"
        );
    }
}
