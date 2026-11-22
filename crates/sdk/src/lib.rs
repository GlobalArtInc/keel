pub mod access;
pub mod backup;
pub mod clock;
pub mod config;
pub mod context;
pub mod dashboard;
pub mod db;
pub mod download;
pub mod error;
pub mod function;
pub mod guard;
pub mod host;
pub mod lifecycle;
pub mod limit;
pub mod list;
pub mod module;
pub mod multipart;
pub mod prelude;
pub mod product;
pub mod registrar;
pub mod secret;
pub mod settings;
pub mod svc;
pub mod task;
pub mod ui;

mod call;
mod instance;
mod mail;
mod services;
mod stat;
mod stream;

#[doc(hidden)]
pub mod runtime;
#[doc(hidden)]
pub mod system;

pub use async_trait;
pub use bytes;
pub use chrono;
pub use chrono_tz;
pub use futures;
pub use keel_types as types;
pub use migrator;
pub use schemars;
pub use sea_orm;
pub use serde;
pub use serde_json;
pub use tokio;

pub use access::{AccountScope, DefaultScopePolicy, Owned, ScopePolicy, scope_by_account};
pub use backup::{BackupDir, BackupSource};
pub use call::{AfterContext, Call};
pub use clock::Clock;
pub use config::ConfigSection;
pub use context::Context;
pub use dashboard::DashboardWidget;
pub use db::Db;
pub use download::{Download, DownloadOutcome};
pub use error::{KeelError, Result};
pub use function::{FunctionBuilder, FunctionDef, function};
pub use guard::Guard;
pub use host::{AgentOp, AgentOpError, AgentScope, ConfigTemplate};
pub use instance::InstanceInfoSource;
pub use keel_types::{
    Account, Choice, Choices, FileAttrs, GroupResult, Id, Idempotency, Level, ListQuery,
    OptionsQuery, Page, Principal,
};
pub use list::{EntitySource, ListSource, ListSpec, entity_source, escape_like, fetch_page};
pub use mail::{MailTemplate, MailText};
pub use module::ModuleDef;
pub use product::{Asset, Brand, Bridge, Edition, LevelLabels, MenuGroup, ProductManifest};
pub use registrar::{Before, DescriptorPatch, MenuNode, MenuSection, Registrar};
pub use secret::Secret;
pub use settings::{SettingDef, SettingsSource};
pub use stat::StatCollector;
pub use stream::Lines;
pub use svc::{ServiceKind, ServiceKindDef};
pub use task::{TaskHandle, TaskOutput};
pub use ui::{Action, ActionScreen, Column, Form, List, ObjectScreen, ViewSpec};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;

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
            grants: BTreeMap::new(),
        }
    }

    pub fn read_handle() -> CallHandle {
        CallBuilder::new(principal(Level::User), "req-1").read(DatabaseConnection::Disconnected)
    }
}
