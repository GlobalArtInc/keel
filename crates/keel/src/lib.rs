mod cli;
mod error;
mod products;
mod runtime;

pub use std::process::ExitCode;

pub use keel_kernel::Config;
pub use keel_sdk as sdk;
pub use keel_sdk::{ModuleDef, ProductBundle};
pub use keel_types as types;

pub use error::Error;
pub use products::{Change, ProductState};
pub use runtime::{Running, Runtime};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
