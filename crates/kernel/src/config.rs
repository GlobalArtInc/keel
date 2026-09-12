use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use sea_orm::sqlx::postgres::PgConnectOptions;
use serde::Deserialize;
use serde_yaml_ng::Value;

pub const FILE_NAME: &str = "config.yaml";
pub const CONFIG_DIR: &str = "/etc/keel";
pub const SOCKET: &str = "/run/keel/core.sock";
pub const DATA_DIR: &str = "/var/lib/keel";
pub const DATABASE: &str = "postgres:///keel?host=/run/postgresql";
pub const PASSWORD_VARIABLE: &str = "KEEL_DB_PASS";
pub const SCHEMA: &str = "keel";
pub const DEFAULT_LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1500);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub listen: SocketAddr,
    pub socket: PathBuf,
    pub data_dir: PathBuf,
    pub database: DatabaseUrl,
    pub tls: Tls,
    pub trusted_proxies: Vec<String>,
    pub log: Log,
    pub sections: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Tls {
    pub cert: String,
    pub key: String,
    pub acme: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Log {
    pub level: String,
    pub format: LogFormat,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::Json,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Text,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not valid configuration: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("the value of \"{key}\" in {FILE_NAME} is unusable: {reason}")]
    Invalid { key: &'static str, reason: String },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Raw {
    listen: Option<String>,
    socket: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    database: Option<String>,
    tls: Option<Tls>,
    trusted_proxies: Option<Vec<String>>,
    log: Option<Log>,
    #[serde(flatten)]
    sections: BTreeMap<String, Value>,
}

impl Config {
    pub fn default_path() -> PathBuf {
        Path::new(CONFIG_DIR).join(FILE_NAME)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let raw = match text {
            Some(text) => Raw::parse(&text).map_err(|source| ConfigError::Parse {
                path: path.to_owned(),
                source,
            })?,
            None => Raw::default(),
        };
        Self::resolve(raw, |name| std::env::var(name).ok())
    }

    pub fn parse(
        text: Option<&str>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let raw = match text {
            Some(text) => Raw::parse(text).map_err(|source| ConfigError::Parse {
                path: PathBuf::from(FILE_NAME),
                source,
            })?,
            None => Raw::default(),
        };
        Self::resolve(raw, env)
    }

    fn resolve(raw: Raw, env: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let listen = match raw.listen {
            Some(text) => text.parse().map_err(|error| ConfigError::Invalid {
                key: "listen",
                reason: format!("{error} ({text:?})"),
            })?,
            None => DEFAULT_LISTEN,
        };
        let database = match raw.database {
            Some(text) => {
                DatabaseUrl::resolve(&text, env).map_err(|error| ConfigError::Invalid {
                    key: "database",
                    reason: error.to_string(),
                })?
            }
            None => DatabaseUrl::default(),
        };
        Ok(Self {
            listen,
            socket: raw.socket.unwrap_or_else(|| PathBuf::from(SOCKET)),
            data_dir: raw.data_dir.unwrap_or_else(|| PathBuf::from(DATA_DIR)),
            database,
            tls: raw.tls.unwrap_or_default(),
            trusted_proxies: raw.trusted_proxies.unwrap_or_default(),
            log: raw.log.unwrap_or_default(),
            sections: raw.sections,
        })
    }
}

impl Raw {
    fn parse(text: &str) -> Result<Self, serde_yaml_ng::Error> {
        match serde_yaml_ng::from_str::<Value>(text)? {
            Value::Null => Ok(Self::default()),
            value => serde_yaml_ng::from_value(value),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseUrl(String);

#[derive(Debug, thiserror::Error)]
pub enum DatabaseUrlError {
    #[error("it is empty")]
    Empty,
    #[error("it must start with postgres://")]
    Scheme,
    #[error("${{{variable}}} is not set in the environment")]
    Unset { variable: String },
    #[error("${{{placeholder}}} is not substituted; only ${{{variable}}} is")]
    Placeholder {
        placeholder: String,
        variable: String,
    },
    #[error("it is not a connection URL: {0}")]
    Malformed(String),
}

impl Default for DatabaseUrl {
    fn default() -> Self {
        Self(DATABASE.to_owned())
    }
}

impl DatabaseUrl {
    pub fn parse(text: &str) -> Result<Self, DatabaseUrlError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(DatabaseUrlError::Empty);
        }
        let normalised = match text.strip_prefix("postgresql://") {
            Some(rest) => format!("postgres://{rest}"),
            None => text.to_owned(),
        };
        if !normalised.starts_with("postgres://") {
            return Err(DatabaseUrlError::Scheme);
        }
        normalised
            .parse::<PgConnectOptions>()
            .map_err(|error| DatabaseUrlError::Malformed(error.to_string()))?;
        Ok(Self(normalised))
    }

    pub fn resolve(
        text: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, DatabaseUrlError> {
        let variable = PASSWORD_VARIABLE.to_owned();
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find('}') else {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            };
            let placeholder = &after[..end];
            if placeholder != variable {
                return Err(DatabaseUrlError::Placeholder {
                    placeholder: placeholder.to_owned(),
                    variable,
                });
            }
            let value = env(&variable).ok_or_else(|| DatabaseUrlError::Unset {
                variable: variable.clone(),
            })?;
            out.push_str(&percent_encode(&value));
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        Self::parse(&out)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn redacted(&self) -> String {
        let Some((scheme, rest)) = self.0.split_once("://") else {
            return self.0.clone();
        };
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(authority_end);
        let authority = match authority.rsplit_once('@') {
            Some((userinfo, host)) => match userinfo.split_once(':') {
                Some((user, _)) => format!("{user}:{MASK}@{host}"),
                None => format!("{userinfo}@{host}"),
            },
            None => authority.to_owned(),
        };
        let tail = match tail.split_once('?') {
            Some((path, query)) => format!("{path}?{}", redact_query(query)),
            None => tail.to_owned(),
        };
        format!("{scheme}://{authority}{tail}")
    }
}

const MASK: &str = "***";

fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some(("password", _)) => format!("password={MASK}"),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

impl fmt::Debug for DatabaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DatabaseUrl({})", self.redacted())
    }
}

impl fmt::Display for DatabaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_missing_config_file_yields_the_platform_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join(FILE_NAME)).unwrap();
        assert_eq!(config.listen, "127.0.0.1:1500".parse().unwrap());
        assert_eq!(config.socket, PathBuf::from("/run/keel/core.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/keel"));
        assert_eq!(
            config.database.expose(),
            "postgres:///keel?host=/run/postgresql"
        );
        assert_eq!(config.tls, Tls::default());
        assert!(config.tls.cert.is_empty());
        assert!(config.tls.key.is_empty());
        assert!(config.tls.acme.is_empty());
        assert!(config.trusted_proxies.is_empty());
        assert_eq!(config.log, Log::default());
        assert_eq!(config.log.level, "info");
        assert_eq!(config.log.format, LogFormat::Json);
        assert!(config.sections.is_empty());
        assert_eq!(
            Config::default_path(),
            PathBuf::from("/etc/keel/config.yaml")
        );
    }

    #[test]
    fn an_empty_file_yields_the_defaults_too() {
        let from_empty = Config::parse(Some(""), no_env).unwrap();
        let from_nothing = Config::parse(None, no_env).unwrap();
        assert_eq!(from_empty, from_nothing);
    }

    #[test]
    fn every_key_of_the_file_overrides_its_default() {
        let text = r#"
listen: 0.0.0.0:9443
socket: /tmp/keel.sock
data_dir: /tmp/keel
database: postgres://keel@db.example.net/keel
tls: { cert: /etc/keel/cert.pem, key: /etc/keel/key.pem, acme: "" }
trusted_proxies: [10.0.0.0/8]
log: { level: debug, format: text }
"#;
        let config = Config::parse(Some(text), no_env).unwrap();
        assert_eq!(config.listen, "0.0.0.0:9443".parse().unwrap());
        assert_eq!(config.socket, PathBuf::from("/tmp/keel.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/tmp/keel"));
        assert_eq!(
            config.database.expose(),
            "postgres://keel@db.example.net/keel"
        );
        assert_eq!(config.tls.cert, "/etc/keel/cert.pem");
        assert_eq!(config.tls.key, "/etc/keel/key.pem");
        assert!(config.tls.acme.is_empty());
        assert_eq!(config.trusted_proxies, ["10.0.0.0/8"]);
        assert_eq!(config.log.level, "debug");
        assert_eq!(config.log.format, LogFormat::Text);
    }

    #[test]
    fn the_password_placeholder_is_substituted_from_the_environment() {
        let text = "database: postgres://keel:${KEEL_DB_PASS}@db.example.net/keel";
        let config = Config::parse(Some(text), |name| {
            (name == "KEEL_DB_PASS").then(|| "s3cret".to_owned())
        })
        .unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel:s3cret@db.example.net/keel"
        );
        assert_eq!(
            config.database.to_string(),
            "postgres://keel:***@db.example.net/keel"
        );
        assert_eq!(
            format!("{:?}", config.database),
            "DatabaseUrl(postgres://keel:***@db.example.net/keel)"
        );
    }

    #[test]
    fn a_password_query_parameter_of_a_tcp_url_is_redacted() {
        let text = "database: postgres://keel@db.example.net/keel?password=${KEEL_DB_PASS}&sslmode=require";
        let config = Config::parse(Some(text), |_| Some("s3cret".to_owned())).unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel@db.example.net/keel?password=s3cret&sslmode=require"
        );
        assert_eq!(
            config.database.to_string(),
            "postgres://keel@db.example.net/keel?password=***&sslmode=require"
        );
        let both =
            DatabaseUrl::parse("postgres://keel:s3cret@db.example.net:5433/keel?password=s3cret")
                .unwrap();
        assert_eq!(
            both.to_string(),
            "postgres://keel:***@db.example.net:5433/keel?password=***"
        );
        assert!(!format!("{both:?}").contains("s3cret"));
    }

    #[test]
    fn a_password_query_parameter_of_a_socket_url_is_redacted() {
        let url =
            DatabaseUrl::parse("postgres:///keel?host=/run/postgresql&password=s3cret").unwrap();
        assert_eq!(
            url.to_string(),
            "postgres:///keel?host=/run/postgresql&password=***"
        );
        let with_at =
            DatabaseUrl::parse("postgres:///keel?password=p@ss&host=/run/postgresql").unwrap();
        assert_eq!(
            with_at.to_string(),
            "postgres:///keel?password=***&host=/run/postgresql"
        );
        assert_eq!(
            DatabaseUrl::default().to_string(),
            "postgres:///keel?host=/run/postgresql"
        );
    }

    #[test]
    fn a_substituted_password_is_percent_encoded_and_decodes_back() {
        let text = "database: postgres://keel:${KEEL_DB_PASS}@db.example.net/keel";
        let config = Config::parse(Some(text), |_| Some("p@ss/w:rd#1".to_owned())).unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel:p%40ss%2Fw%3Ard%231@db.example.net/keel"
        );
        let options: PgConnectOptions = config.database.expose().parse().unwrap();
        assert_eq!(options.get_username(), "keel");
        assert_eq!(options.get_database(), Some("keel"));
    }

    #[test]
    fn an_unset_password_variable_fails_naming_the_key() {
        let text = "database: postgres://keel:${KEEL_DB_PASS}@db.example.net/keel";
        let error = Config::parse(Some(text), no_env).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "database",
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("\"database\""), "{message}");
        assert!(message.contains("KEEL_DB_PASS"), "{message}");
    }

    #[test]
    fn a_foreign_placeholder_is_refused() {
        let text = "database: postgres://keel:${OTHER}@db.example.net/keel";
        let error = Config::parse(Some(text), |_| Some("x".to_owned())).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("\"database\""), "{message}");
        assert!(message.contains("${OTHER}"), "{message}");
        assert!(message.contains("${KEEL_DB_PASS}"), "{message}");
    }

    #[test]
    fn an_unusable_database_value_fails_naming_the_key() {
        for text in [
            "database: \"\"",
            "database: mysql://keel@localhost/keel",
            "database: not a url at all",
            "database: postgres://keel@localhost:notaport/keel",
        ] {
            let error = Config::parse(Some(text), no_env).unwrap_err();
            assert!(
                matches!(
                    error,
                    ConfigError::Invalid {
                        key: "database",
                        ..
                    }
                ),
                "{text}: {error}"
            );
            assert!(
                error.to_string().contains("\"database\""),
                "{text}: {error}"
            );
        }
    }

    #[test]
    fn a_postgresql_scheme_is_accepted_and_normalised() {
        let url = DatabaseUrl::parse("postgresql://keel@localhost/keel").unwrap();
        assert_eq!(url.expose(), "postgres://keel@localhost/keel");
        assert_eq!(url.to_string(), url.expose());
    }

    #[test]
    fn an_unusable_listen_value_fails_naming_the_key() {
        let error = Config::parse(Some("listen: nowhere"), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::Invalid { key: "listen", .. }));
        assert!(error.to_string().contains("\"listen\""));
    }

    #[test]
    fn a_typo_inside_a_known_section_is_an_error() {
        let error = Config::parse(Some("log: { levle: debug }"), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
        assert!(error.to_string().contains("levle"), "{error}");
    }

    #[test]
    fn unknown_top_level_sections_are_kept_for_modules() {
        let text = "host:\n  socket: /run/keel/agent.sock\n";
        let config = Config::parse(Some(text), no_env).unwrap();
        assert_eq!(config.sections.len(), 1);
        assert_eq!(
            config.sections["host"]["socket"],
            Value::String("/run/keel/agent.sock".to_owned())
        );
    }

    #[test]
    fn an_unreadable_file_that_exists_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = Config::load(dir.path()).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    }
}
