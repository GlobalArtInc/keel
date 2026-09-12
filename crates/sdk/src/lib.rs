pub mod access;
pub mod clock;
pub mod context;
pub mod db;
pub mod error;
pub mod function;
pub mod guard;
pub mod list;
pub mod module;
pub mod prelude;
pub mod product;
pub mod registrar;
pub mod secret;
pub mod ui;

mod call;
mod services;

#[doc(hidden)]
pub mod runtime;
#[doc(hidden)]
pub mod system;

pub use async_trait;
pub use chrono;
pub use keel_types as types;
pub use migrator;
pub use schemars;
pub use sea_orm;
pub use serde;
pub use serde_json;

pub use access::{AccountScope, DefaultScopePolicy, Owned, ScopePolicy, scope_by_account};
pub use call::{AfterContext, Call};
pub use clock::Clock;
pub use context::Context;
pub use db::Db;
pub use error::{KeelError, Result};
pub use function::{FunctionBuilder, FunctionDef, function};
pub use guard::Guard;
pub use keel_types::{
    Account, Choice, Choices, Id, Level, ListQuery, OptionsQuery, Page, Principal,
};
pub use list::{EntitySource, ListSource, ListSpec, entity_source, escape_like};
pub use module::ModuleDef;
pub use product::{Brand, Edition, LevelLabels, MenuGroup, ProductManifest};
pub use registrar::{Before, DescriptorPatch, MenuNode, Registrar};
pub use secret::Secret;
pub use ui::{Action, Column, Form, List};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
pub(crate) mod tests {
    use keel_types::{Level, Principal, Scope, Via};
    use sea_orm::DatabaseConnection;

    use crate::call::{CallBuilder, CallHandle};

    pub fn principal(level: Level) -> Principal {
        Principal {
            id: "01J9Q3W6B1R8N5X2K4T7M0V9A1".parse().unwrap(),
            login: "alice".into(),
            level,
            account_id: "01J8A0000000000000000000A1".parse().unwrap(),
            role: "owner".into(),
            actor: None,
            scope: Scope::Full,
            via: Via::Http,
            external_id: None,
            origin: None,
            lang: "en".into(),
        }
    }

    pub fn read_handle() -> CallHandle {
        CallBuilder::new(principal(Level::User), "req-1").read(DatabaseConnection::Disconnected)
    }
}
