#[cfg(feature = "audit")]
pub mod audit;
#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "backup")]
pub mod backup;
#[cfg(feature = "cron")]
pub mod cron;
#[cfg(feature = "license")]
pub mod license;
#[cfg(feature = "notify")]
pub mod notify;
#[cfg(feature = "peers")]
pub mod peers;
#[cfg(feature = "settings")]
pub mod settings;
#[cfg(feature = "stats")]
pub mod stats;
#[cfg(feature = "tasks")]
pub mod tasks;
#[cfg(feature = "update")]
pub mod update;
#[cfg(feature = "webhooks")]
pub mod webhooks;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
