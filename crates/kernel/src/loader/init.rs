use std::fmt;

use keel_sdk::runtime::{RegistrationError, Registrations};
use keel_sdk::{KeelError, Registrar};

use super::composition::Graph;

pub struct Initialised {
    pub(super) graph: Graph,
    pub(super) registrations: Vec<Registrations>,
}

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("module {module}: init failed: {source}")]
    Init {
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
        let mut registrations = Vec::with_capacity(self.modules().len());
        for def in self.modules() {
            let mut registrar = Registrar::new(def.id);
            (def.init)(&mut registrar).map_err(|source| InitError::Init {
                module: def.id.to_owned(),
                source,
            })?;
            let registered =
                registrar
                    .into_registrations()
                    .map_err(|errors| InitError::Registration {
                        module: def.id.to_owned(),
                        errors,
                    })?;
            tracing::debug!(
                module = def.id,
                functions = registered.functions.len(),
                "module initialised"
            );
            registrations.push(registered);
        }
        Ok(Initialised {
            graph: self.clone(),
            registrations,
        })
    }
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
