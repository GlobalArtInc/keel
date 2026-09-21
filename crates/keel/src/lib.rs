mod backup;
mod cli;
mod error;
mod privileged;
mod products;
mod runtime;

pub use std::process::ExitCode;

pub use keel_kernel::http::Frontend;
pub use keel_kernel::{Config, TlsMaterial};
pub use keel_sdk as sdk;
pub use keel_sdk::prelude;
pub use keel_sdk::{ModuleDef, ProductManifest};
pub use keel_types as types;

pub use error::Error;
pub use products::{Change, ProductState};
pub use runtime::{Running, Runtime};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[doc(hidden)]
pub use rust_embed as __embed;

#[macro_export]
macro_rules! embed_frontend {
    ($folder:literal) => {{
        use $crate::__embed as rust_embed;

        #[derive(rust_embed::Embed)]
        #[folder = $folder]
        struct Shell;

        $crate::Frontend::embedded::<Shell>()
    }};
}
