use std::error::Error;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::filter::ParseError;

use crate::config::{Log, LogFormat};

#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    #[error("the value of \"log.level\" is not a log filter: {0}")]
    Level(#[from] ParseError),
    #[error("logging could not be initialised: {0}")]
    Init(#[source] Box<dyn Error + Send + Sync>),
}

fn filter(log: &Log) -> Result<EnvFilter, LoggingError> {
    Ok(EnvFilter::try_new(&log.level)?)
}

pub fn init(log: &Log) -> Result<(), LoggingError> {
    let builder = tracing_subscriber::fmt().with_env_filter(filter(log)?);
    match log.format {
        LogFormat::Json => builder.json().try_init(),
        LogFormat::Text => builder.try_init(),
    }
    .map_err(LoggingError::Init)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_log_key_is_a_valid_filter() {
        assert_eq!(filter(&Log::default()).unwrap().to_string(), "info");
    }

    #[test]
    fn a_per_target_level_is_accepted() {
        let log = Log {
            level: "info,keel_kernel=debug".to_owned(),
            format: LogFormat::Text,
        };
        let filter = filter(&log).unwrap();
        assert!(filter.to_string().contains("keel_kernel=debug"));
    }

    #[test]
    fn a_broken_level_fails_naming_the_key() {
        let log = Log {
            level: "info,=".to_owned(),
            format: LogFormat::Json,
        };
        let error = filter(&log).unwrap_err();
        assert!(matches!(error, LoggingError::Level(_)));
        assert!(error.to_string().contains("log.level"), "{error}");
    }

    #[test]
    fn logging_initialises_once_per_process() {
        let log = Log {
            level: "warn".to_owned(),
            format: LogFormat::Text,
        };
        init(&log).unwrap();
        assert!(matches!(init(&log), Err(LoggingError::Init(_))));
    }
}
