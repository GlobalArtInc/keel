mod composition;
mod freeze;
mod i18n;
mod init;
mod keys;
mod lifecycle;
mod migrate;
mod patch;
mod section;
mod settings;

pub use composition::{Composition, CompositionError, Graph, LEDGER_PREFIX, Problem, Sections};
pub use freeze::{FreezeError, FreezeProblem, Frozen};
pub use i18n::REQUIRED_LANGUAGE;
pub use init::{InitError, Initialised};
pub use lifecycle::{Kernel, StartError, StopError};
pub use migrate::{ACTIVE, MigrateError, Migrated, ORPHANED};

use crate::storage::Storage;

#[derive(Debug, thiserror::Error)]
pub enum BootError {
    #[error(transparent)]
    Composition(#[from] CompositionError),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error(transparent)]
    Init(#[from] InitError),
    #[error(transparent)]
    Freeze(#[from] FreezeError),
    #[error(transparent)]
    Start(#[from] StartError),
}

impl Kernel {
    pub async fn boot(composition: Composition, storage: &Storage) -> Result<Kernel, BootError> {
        let graph = composition.resolve()?;
        graph.migrate(storage).await?;
        let frozen = graph.init()?.freeze()?;
        Ok(frozen.start().await?)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use keel_sdk::prelude::*;

    #[derive(Deserialize, Serialize, JsonSchema)]
    pub struct Nothing {}

    #[derive(Deserialize, Serialize, JsonSchema)]
    pub struct Item {
        pub id: Option<Id>,
        pub title: String,
        pub kind: String,
        pub weight: i32,
    }

    pub fn get(name: &str) -> FunctionDef {
        function::<Nothing, Nothing>(name)
            .kind(Kind::Get)
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
    }

    pub fn options(name: &str) -> FunctionDef {
        function::<Nothing, Nothing>(name)
            .kind(Kind::Options)
            .handler(|_: Call<Nothing>| async { Ok(Nothing {}) })
    }

    pub fn form(name: &str, ui: Form) -> FunctionDef {
        function::<Item, Nothing>(name)
            .kind(Kind::Form)
            .level(Level::User)
            .unowned()
            .ui(ui)
            .handler(|_: Call<Item>| async { Ok(Nothing {}) })
    }

    macro_rules! module {
        ($name:ident, $id:literal, [$($dep:literal),*], $init:expr) => {
            static $name: keel_sdk::ModuleDef = keel_sdk::ModuleDef {
                id: $id,
                version: "0.1.0",
                sdk: keel_sdk::VERSION,
                requires: &[$($dep),*],
                migrations: None,
                init: $init,
            };
        };
    }

    pub(crate) use module;
}
