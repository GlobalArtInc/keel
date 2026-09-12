use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use sea_orm::sqlx::postgres::PgConnectOptions;
use serde::Deserialize;
use serde_yaml_ng::Value;

pub const FILE_NAME: &str = "config.yaml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestDefaults {
    pub product: String,
    pub listen: SocketAddr,
    pub socket: PathBuf,
    pub data_dir: PathBuf,
}

impl ManifestDefaults {
    pub fn standard(product: &str, listen: SocketAddr) -> Self {
        Self {
            product: product.to_owned(),
            listen,
            socket: PathBuf::from(format!("/run/keel/{product}/core.sock")),
            data_dir: PathBuf::from(format!("/var/lib/keel/{product}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub product: String,
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
    #[error(
        "\"{0}\" is not a product id: it must start with a letter and contain only lowercase letters, digits and underscores"
    )]
    Product(String),
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
    pub fn default_path(product: &str) -> PathBuf {
        PathBuf::from(format!("/etc/keel/{product}/{FILE_NAME}"))
    }

    pub fn load(path: &Path, defaults: &ManifestDefaults) -> Result<Self, ConfigError> {
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
        Self::resolve(raw, defaults, |name| std::env::var(name).ok())
    }

    pub fn parse(
        text: Option<&str>,
        defaults: &ManifestDefaults,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let raw = match text {
            Some(text) => Raw::parse(text).map_err(|source| ConfigError::Parse {
                path: PathBuf::from(FILE_NAME),
                source,
            })?,
            None => Raw::default(),
        };
        Self::resolve(raw, defaults, env)
    }

    pub fn password_variable(product: &str) -> String {
        format!("{}_DB_PASS", product.to_ascii_uppercase())
    }

    fn resolve(
        raw: Raw,
        defaults: &ManifestDefaults,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let product = defaults.product.clone();
        if !is_product_id(&product) {
            return Err(ConfigError::Product(product));
        }
        let listen = match raw.listen {
            Some(text) => text.parse().map_err(|error| ConfigError::Invalid {
                key: "listen",
                reason: format!("{error} ({text:?})"),
            })?,
            None => defaults.listen,
        };
        let database = match raw.database {
            Some(text) => DatabaseUrl::resolve(&text, &product, env).map_err(|error| {
                ConfigError::Invalid {
                    key: "database",
                    reason: error.to_string(),
                }
            })?,
            None => DatabaseUrl::default_for(&product),
        };
        Ok(Self {
            product,
            listen,
            socket: raw.socket.unwrap_or_else(|| defaults.socket.clone()),
            data_dir: raw.data_dir.unwrap_or_else(|| defaults.data_dir.clone()),
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

fn is_product_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

impl DatabaseUrl {
    pub fn default_for(product: &str) -> Self {
        Self(format!("postgres:///keel_{product}?host=/run/postgresql"))
    }

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
        product: &str,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, DatabaseUrlError> {
        let variable = Config::password_variable(product);
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

    fn defaults() -> ManifestDefaults {
        ManifestDefaults::standard("demo", "127.0.0.1:8443".parse().unwrap())
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_missing_config_file_yields_the_manifest_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join(FILE_NAME), &defaults()).unwrap();
        assert_eq!(config.product, "demo");
        assert_eq!(config.listen, "127.0.0.1:8443".parse().unwrap());
        assert_eq!(config.socket, PathBuf::from("/run/keel/demo/core.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/keel/demo"));
        assert_eq!(
            config.database.expose(),
            "postgres:///keel_demo?host=/run/postgresql"
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
    }

    #[test]
    fn an_empty_file_yields_the_defaults_too() {
        let from_empty = Config::parse(Some(""), &defaults(), no_env).unwrap();
        let from_nothing = Config::parse(None, &defaults(), no_env).unwrap();
        assert_eq!(from_empty, from_nothing);
    }

    #[test]
    fn every_key_of_the_file_overrides_its_default() {
        let text = r#"
listen: 0.0.0.0:9443
socket: /tmp/demo.sock
data_dir: /tmp/demo
database: postgres://keel_demo@db.example.net/keel_demo
tls: { cert: /etc/demo/cert.pem, key: /etc/demo/key.pem, acme: "" }
trusted_proxies: [10.0.0.0/8]
log: { level: debug, format: text }
"#;
        let config = Config::parse(Some(text), &defaults(), no_env).unwrap();
        assert_eq!(config.listen, "0.0.0.0:9443".parse().unwrap());
        assert_eq!(config.socket, PathBuf::from("/tmp/demo.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/tmp/demo"));
        assert_eq!(
            config.database.expose(),
            "postgres://keel_demo@db.example.net/keel_demo"
        );
        assert_eq!(config.tls.cert, "/etc/demo/cert.pem");
        assert_eq!(config.tls.key, "/etc/demo/key.pem");
        assert!(config.tls.acme.is_empty());
        assert_eq!(config.trusted_proxies, ["10.0.0.0/8"]);
        assert_eq!(config.log.level, "debug");
        assert_eq!(config.log.format, LogFormat::Text);
    }

    #[test]
    fn the_password_placeholder_is_substituted_from_the_environment() {
        let text = "database: postgres://keel_demo:${DEMO_DB_PASS}@db.example.net/keel_demo";
        let config = Config::parse(Some(text), &defaults(), |name| {
            (name == "DEMO_DB_PASS").then(|| "s3cret".to_owned())
        })
        .unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel_demo:s3cret@db.example.net/keel_demo"
        );
        assert_eq!(
            config.database.to_string(),
            "postgres://keel_demo:***@db.example.net/keel_demo"
        );
        assert_eq!(
            format!("{:?}", config.database),
            "DatabaseUrl(postgres://keel_demo:***@db.example.net/keel_demo)"
        );
    }

    #[test]
    fn a_password_query_parameter_of_a_tcp_url_is_redacted() {
        let text = "database: postgres://keel_demo@db.example.net/keel_demo?password=${DEMO_DB_PASS}&sslmode=require";
        let config = Config::parse(Some(text), &defaults(), |_| Some("s3cret".to_owned())).unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel_demo@db.example.net/keel_demo?password=s3cret&sslmode=require"
        );
        assert_eq!(
            config.database.to_string(),
            "postgres://keel_demo@db.example.net/keel_demo?password=***&sslmode=require"
        );
        let both = DatabaseUrl::parse(
            "postgres://keel_demo:s3cret@db.example.net:5433/keel_demo?password=s3cret",
        )
        .unwrap();
        assert_eq!(
            both.to_string(),
            "postgres://keel_demo:***@db.example.net:5433/keel_demo?password=***"
        );
        assert!(!format!("{both:?}").contains("s3cret"));
    }

    #[test]
    fn a_password_query_parameter_of_a_socket_url_is_redacted() {
        let url = DatabaseUrl::parse("postgres:///keel_demo?host=/run/postgresql&password=s3cret")
            .unwrap();
        assert_eq!(
            url.to_string(),
            "postgres:///keel_demo?host=/run/postgresql&password=***"
        );
        let with_at =
            DatabaseUrl::parse("postgres:///keel_demo?password=p@ss&host=/run/postgresql").unwrap();
        assert_eq!(
            with_at.to_string(),
            "postgres:///keel_demo?password=***&host=/run/postgresql"
        );
        assert_eq!(
            DatabaseUrl::default_for("demo").to_string(),
            "postgres:///keel_demo?host=/run/postgresql"
        );
    }

    #[test]
    fn a_substituted_password_is_percent_encoded_and_decodes_back() {
        let text = "database: postgres://keel_demo:${DEMO_DB_PASS}@db.example.net/keel_demo";
        let config =
            Config::parse(Some(text), &defaults(), |_| Some("p@ss/w:rd#1".to_owned())).unwrap();
        assert_eq!(
            config.database.expose(),
            "postgres://keel_demo:p%40ss%2Fw%3Ard%231@db.example.net/keel_demo"
        );
        let options: PgConnectOptions = config.database.expose().parse().unwrap();
        assert_eq!(options.get_username(), "keel_demo");
        assert_eq!(options.get_database(), Some("keel_demo"));
    }

    #[test]
    fn an_unset_password_variable_fails_naming_the_key() {
        let text = "database: postgres://keel_demo:${DEMO_DB_PASS}@db.example.net/keel_demo";
        let error = Config::parse(Some(text), &defaults(), no_env).unwrap_err();
        assert!(matches!(
            error,
            ConfigError::Invalid {
                key: "database",
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("\"database\""), "{message}");
        assert!(message.contains("DEMO_DB_PASS"), "{message}");
    }

    #[test]
    fn a_foreign_placeholder_is_refused() {
        let text = "database: postgres://keel_demo:${OTHER}@db.example.net/keel_demo";
        let error = Config::parse(Some(text), &defaults(), |_| Some("x".to_owned())).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("\"database\""), "{message}");
        assert!(message.contains("${OTHER}"), "{message}");
        assert!(message.contains("${DEMO_DB_PASS}"), "{message}");
    }

    #[test]
    fn an_unusable_database_value_fails_naming_the_key() {
        for text in [
            "database: \"\"",
            "database: mysql://keel_demo@localhost/keel_demo",
            "database: not a url at all",
            "database: postgres://keel_demo@localhost:notaport/keel_demo",
        ] {
            let error = Config::parse(Some(text), &defaults(), no_env).unwrap_err();
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
        let url = DatabaseUrl::parse("postgresql://keel_demo@localhost/keel_demo").unwrap();
        assert_eq!(url.expose(), "postgres://keel_demo@localhost/keel_demo");
        assert_eq!(url.to_string(), url.expose());
    }

    #[test]
    fn an_unusable_listen_value_fails_naming_the_key() {
        let error = Config::parse(Some("listen: nowhere"), &defaults(), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::Invalid { key: "listen", .. }));
        assert!(error.to_string().contains("\"listen\""));
    }

    #[test]
    fn a_typo_inside_a_known_section_is_an_error() {
        let error = Config::parse(Some("log: { levle: debug }"), &defaults(), no_env).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{error}");
        assert!(error.to_string().contains("levle"), "{error}");
    }

    #[test]
    fn unknown_top_level_sections_are_kept_for_modules() {
        let text = "host:\n  socket: /run/keel/demo/agent.sock\n";
        let config = Config::parse(Some(text), &defaults(), no_env).unwrap();
        assert_eq!(config.sections.len(), 1);
        assert_eq!(
            config.sections["host"]["socket"],
            Value::String("/run/keel/demo/agent.sock".to_owned())
        );
    }

    #[test]
    fn an_unreadable_file_that_exists_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = Config::load(dir.path(), &defaults()).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }), "{error}");
    }

    #[test]
    fn a_product_id_that_is_not_an_identifier_is_refused() {
        for bad in ["", "Demo", "1demo", "de-mo", "de.mo", "demo;"] {
            let defaults = ManifestDefaults::standard(bad, "127.0.0.1:1".parse().unwrap());
            let error = Config::parse(None, &defaults, no_env).unwrap_err();
            assert!(matches!(error, ConfigError::Product(_)), "{bad:?}: {error}");
        }
        for good in ["demo", "dns", "panel", "bill", "keel_vm", "a1"] {
            let defaults = ManifestDefaults::standard(good, "127.0.0.1:1".parse().unwrap());
            assert!(Config::parse(None, &defaults, no_env).is_ok(), "{good}");
        }
    }

    #[test]
    fn the_password_variable_and_the_default_path_follow_the_product() {
        assert_eq!(Config::password_variable("dns"), "DNS_DB_PASS");
        assert_eq!(
            Config::default_path("dns"),
            PathBuf::from("/etc/keel/dns/config.yaml")
        );
        assert_eq!(
            DatabaseUrl::default_for("dns").expose(),
            "postgres:///keel_dns?host=/run/postgresql"
        );
    }
}
