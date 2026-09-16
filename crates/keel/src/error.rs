use std::path::PathBuf;

use keel_kernel::ctl::CtlError;
use keel_kernel::instance::InstanceError;
use keel_kernel::loader::{BootError, CompositionError, MigrateError, StopError};
use keel_kernel::logging::LoggingError;
use keel_kernel::products::ProductsError;
use keel_kernel::{ConfigError, HttpError, SocketError, StorageError, TlsError};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Logging(#[from] LoggingError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error(transparent)]
    Products(#[from] ProductsError),
    #[error(transparent)]
    Composition(#[from] CompositionError),
    #[error(transparent)]
    Migrate(#[from] MigrateError),
    #[error(transparent)]
    Boot(#[from] BootError),
    #[error("the dispatcher could not compile a schema: {0}")]
    Schema(#[from] keel_kernel::dispatcher::SchemaError),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Socket(#[from] SocketError),
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error(transparent)]
    Ctl(#[from] CtlError),
    #[error("the product {id} is not carried by this binary; it carries {}", known_list(.known))]
    UnknownProduct { id: String, known: Vec<String> },
    #[error("the service is running on {socket}; stop it before changing the enabled products")]
    ServiceRunning { socket: PathBuf },
    #[error("the runtime could not start its executor: {0}")]
    Executor(#[source] std::io::Error),
}

fn known_list(known: &[String]) -> String {
    if known.is_empty() {
        "no product bundle".to_owned()
    } else {
        known.join(", ")
    }
}

pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str("\n  caused by: ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_product_names_the_id_and_the_known_ids() {
        let error = Error::UnknownProduct {
            id: "x".into(),
            known: vec!["demo".into(), "dns".into()],
        };
        assert_eq!(
            error.to_string(),
            "the product x is not carried by this binary; it carries demo, dns"
        );
        let bare = Error::UnknownProduct {
            id: "x".into(),
            known: Vec::new(),
        };
        assert_eq!(
            bare.to_string(),
            "the product x is not carried by this binary; it carries no product bundle"
        );
    }

    #[test]
    fn the_description_of_an_error_walks_its_causes() {
        let error = Error::Executor(std::io::Error::other("no threads"));
        assert_eq!(
            describe(&error),
            "the runtime could not start its executor: no threads\n  caused by: no threads"
        );
    }
}
