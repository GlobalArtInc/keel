use std::path::PathBuf;

use keel_host_agent::{Failure, Operation, OperationCall, Template};
use keel_kernel::loader::BootError;
use keel_sdk::serde_json::Value;
use keel_sdk::{AgentOp, AgentOpError, AgentScope, ConfigTemplate};

use crate::backup::throwaway_key;
use crate::error::Error;
use crate::runtime::Runtime;

pub(crate) struct Catalogue {
    pub(crate) operations: Vec<Operation>,
    pub(crate) templates: Vec<Template>,
}

impl Runtime {
    pub(crate) fn host_catalogue(&self) -> Result<Catalogue, Error> {
        let key = throwaway_key().map_err(|error| Error::Boot(BootError::Restore(error)))?;
        let frozen = self
            .composition(&self.known_ids())?
            .key(key)
            .resolve()?
            .init()
            .map_err(BootError::from)?
            .freeze()
            .map_err(BootError::from)?;
        let contributions = frozen.registry().contributions();
        Ok(Catalogue {
            operations: contributions
                .of::<AgentOp>()
                .cloned()
                .map(operation)
                .collect(),
            templates: contributions.of::<ConfigTemplate>().map(template).collect(),
        })
    }
}

struct Scope<'a, 'b>(&'a OperationCall<'b>);

impl AgentScope for Scope<'_, '_> {
    fn product(&self) -> &str {
        self.0.product()
    }

    fn policy(&self) -> &Value {
        self.0.policy()
    }

    fn resolve(&self, root: &str, name: &str) -> Result<PathBuf, AgentOpError> {
        self.0
            .resolve(root, name)
            .map_err(|failure| AgentOpError::new(failure.key(), failure.detail()))
    }
}

fn operation(op: AgentOp) -> Operation {
    let name = op.name().to_owned();
    let class = op.class().to_owned();
    Operation::new(&name, &class, move |call, args| {
        op.run(&Scope(call), args)
            .map_err(|error| Failure::new(error.key(), error.detail()))
    })
}

fn template(template: &ConfigTemplate) -> Template {
    Template::new(template.id(), template.text(), template.check())
}
