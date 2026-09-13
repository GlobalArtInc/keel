#[cfg(feature = "audit")]
pub mod audit;
#[cfg(feature = "auth")]
pub mod auth;
#[cfg(feature = "settings")]
pub mod settings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
