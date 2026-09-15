#[cfg(feature = "audit")]
pub mod audit;
#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "settings")]
pub mod settings;
#[cfg(feature = "tasks")]
pub mod tasks;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
