use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use keel_kernel::backup::BackupError;
use keel_kernel::doctor::{Database, Doctor, Layout, Report, ledger_status};
use keel_kernel::keel::LOGS_TAIL;
use keel_kernel::loader::{BootError, Graph, Sections};
use keel_kernel::socket::Client;
use keel_kernel::support::{self, Bundle, Masks, SupportError, Written};
use keel_kernel::{Config, Storage, logs, products, tls};
use keel_types::Envelope;
use serde_json::{Value, json};

use crate::backup::throwaway_key;
use crate::error::{Error, describe};
use crate::runtime::Runtime;

const POLICY_FILE: &str = "agent.yaml";
const SUPPORT_DIR: &str = "support";
const HOST_SECTION: &str = "host";
const HOST_SOCKET: &str = "socket";

pub(crate) fn agent_socket(config: &Config) -> Option<PathBuf> {
    config
        .sections
        .get(HOST_SECTION)?
        .get(HOST_SOCKET)?
        .as_str()
        .map(PathBuf::from)
}
const READINESS_PATH: &str = "/readyz";
const READINESS_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_DIR: &str = "tls";
const CERT_FILE: &str = "cert.pem";

#[derive(Debug, thiserror::Error)]
pub(crate) enum SupportCliError {
    #[error(transparent)]
    Runtime(Box<Error>),
    #[error(transparent)]
    Support(#[from] SupportError),
    #[error(transparent)]
    Key(#[from] BackupError),
}

impl From<Error> for SupportCliError {
    fn from(error: Error) -> Self {
        Self::Runtime(Box::new(error))
    }
}

struct Connected {
    storage: Option<Storage>,
    failure: Option<String>,
}

impl Connected {
    async fn open(config: &Config) -> Self {
        match Storage::connect(config).await {
            Ok(storage) => Self {
                storage: Some(storage),
                failure: None,
            },
            Err(error) => Self {
                storage: None,
                failure: Some(describe(&error)),
            },
        }
    }

    fn database(&self) -> Database<'_> {
        match &self.storage {
            Some(storage) => Database::Connected(storage.db()),
            None => Database::Unreachable(self.failure.clone().unwrap_or_default()),
        }
    }

    async fn close(self) -> Result<(), Error> {
        match self.storage {
            Some(storage) => Ok(storage.close().await?),
            None => Ok(()),
        }
    }
}

impl Runtime {
    async fn site_graph(&self, config: &Config, connected: &Connected) -> Result<Graph, Error> {
        let enabled: Vec<String> = match &connected.storage {
            Some(storage) => products::enabled(storage.db())
                .await
                .map(|set| set.into_keys().collect())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let usable: Vec<String> = enabled
            .into_iter()
            .filter(|id| self.bundle(id).is_some())
            .collect();
        self.graph(config, &usable)
    }

    fn graph(&self, config: &Config, products: &[String]) -> Result<Graph, Error> {
        Ok(self
            .composition(products)?
            .key(throwaway_key().map_err(|error| Error::Boot(BootError::Restore(error)))?)
            .sections(Sections::new(&config.file, config.sections.clone()))
            .data_dir(config.data_dir.clone())
            .resolve()?)
    }

    fn masks(&self, config: &Config) -> Result<Masks, Error> {
        let frozen = self
            .graph(config, &self.known_ids())?
            .init()
            .map_err(BootError::from)?
            .freeze()
            .map_err(BootError::from)?;
        Ok(Masks::of(frozen.registry().contributions()))
    }

    pub(crate) async fn doctor(&self, config: &Config, layout: Layout) -> Result<Report, Error> {
        let connected = Connected::open(config).await;
        let outcome = async {
            let graph = self.site_graph(config, &connected).await?;
            let doctor = Doctor::new(&graph, &config.tls, layout);
            Ok(doctor.run(&connected.database()).await)
        }
        .await;
        connected.close().await?;
        outcome
    }

    pub(crate) async fn support_bundle(
        &self,
        config: &Config,
        layout: Layout,
        out: Option<&Path>,
    ) -> Result<Written, SupportCliError> {
        let connected = Connected::open(config).await;
        let outcome = self.collect(config, layout, out, &connected).await;
        connected.close().await?;
        outcome
    }

    async fn collect(
        &self,
        config: &Config,
        layout: Layout,
        out: Option<&Path>,
        connected: &Connected,
    ) -> Result<Written, SupportCliError> {
        let graph = self.site_graph(config, connected).await?;
        let (masks, mask_note) = match self.masks(config) {
            Ok(masks) => (masks, None),
            Err(_) => (
                Masks::none(),
                Some(
                    "the composition of every carried product did not freeze, so every value of every module section was masked"
                        .to_owned(),
                ),
            ),
        };
        let doctor = Doctor::new(&graph, &config.tls, layout);
        let report = doctor.run(&connected.database()).await;
        let instance = std::fs::read_to_string(
            config
                .data_dir
                .join(keel_kernel::instance::INSTANCE_ID_FILE),
        )
        .ok()
        .map(|text| text.trim().to_owned());
        let mut bundle = Bundle::new(crate::VERSION, report.at, instance);
        if let Some(note) = mask_note {
            bundle.note(support::CONFIG_FILE, note);
        }
        match support::redacted_file(&config.file, &masks)? {
            Some(text) => bundle.add_text(support::CONFIG_FILE, &text),
            None => bundle.note(
                support::CONFIG_FILE,
                format!("{} is absent; the defaults apply", config.file.display()),
            ),
        }
        let policy = policy_path(config);
        match support::read_optional(&policy)? {
            Some(bytes) => bundle.add(support::POLICY_FILE, bytes),
            None => bundle.note(
                support::POLICY_FILE,
                format!("{} is absent", policy.display()),
            ),
        }
        match logs_over_socket(&config.socket).await {
            Ok(entries) => {
                bundle.note(
                    support::LOGS_FILE,
                    format!(
                        "{} entries from the ring of the running service over {}",
                        entries.len(),
                        config.socket.display()
                    ),
                );
                bundle.add_lines(support::LOGS_FILE, &entries);
            }
            Err(reason) => {
                bundle.note(
                    support::LOGS_FILE,
                    format!(
                        "the service did not answer on {}: {reason}; the ring lives in the service process",
                        config.socket.display()
                    ),
                );
                bundle.add_lines(support::LOGS_FILE, &[]);
            }
        }
        match &connected.storage {
            Some(storage) => {
                let db = storage.db();
                let ledgers = ledger_status(db, doctor.ledgers())
                    .await
                    .map_err(|source| SupportError::Database {
                        step: "the migration ledgers",
                        source,
                    })?;
                bundle.add_json(support::LEDGERS_FILE, &ledgers)?;
                match support::audit_tail(db).await? {
                    Some(rows) => bundle.add_lines(support::AUDIT_FILE, &rows),
                    None => bundle.note(
                        support::AUDIT_FILE,
                        "no audit table; the audit module is not loaded",
                    ),
                }
                match support::open_problems(db).await? {
                    Some(rows) => bundle.add_json(support::PROBLEMS_FILE, &rows)?,
                    None => bundle.note(
                        support::PROBLEMS_FILE,
                        "no problem table; the notify module is not loaded",
                    ),
                }
            }
            None => {
                let reason = connected.failure.clone().unwrap_or_default();
                for name in [
                    support::LEDGERS_FILE,
                    support::AUDIT_FILE,
                    support::PROBLEMS_FILE,
                ] {
                    bundle.note(name, format!("the database is unreachable: {reason}"));
                }
            }
        }
        bundle.add_json(support::READINESS_FILE, &readiness(config).await)?;
        bundle.note(
            support::READINESS_FILE,
            "the kernel exposes no metrics yet; the readiness answer stands in",
        );
        bundle.add_text(support::DOCTOR_FILE, &report.render());
        let archive = out.map_or_else(
            || {
                config
                    .data_dir
                    .join(SUPPORT_DIR)
                    .join(support::archive_name(crate::VERSION, report.at))
            },
            Path::to_path_buf,
        );
        Ok(bundle.write(&archive)?)
    }
}

fn policy_path(config: &Config) -> PathBuf {
    config
        .file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from(POLICY_FILE), |dir| dir.join(POLICY_FILE))
}

async fn logs_over_socket(socket: &Path) -> Result<Vec<Value>, String> {
    let mut client = Client::connect(socket)
        .await
        .map_err(|error| error.to_string())?;
    let envelope = client
        .call(
            LOGS_TAIL,
            json!({ "limit": logs::CAPACITY }),
            Default::default(),
        )
        .await
        .map_err(|error| error.to_string())?;
    match envelope {
        Envelope::Ok { data, .. } => Ok(data["entries"].as_array().cloned().unwrap_or_default()),
        Envelope::Err { error } => Err(format!("{LOGS_TAIL} answered {}", error.key)),
    }
}

fn readiness_address(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}

fn certificate_path(config: &Config) -> PathBuf {
    let configured = config.tls.cert.trim();
    if configured.is_empty() {
        config.data_dir.join(TLS_DIR).join(CERT_FILE)
    } else {
        PathBuf::from(configured)
    }
}

async fn readiness(config: &Config) -> Value {
    let address = readiness_address(config.listen);
    let url = format!("https://{address}{READINESS_PATH}");
    match fetch_readiness(config, &url).await {
        Ok((status, body)) => json!({ "url": url, "status": status, "body": body }),
        Err(error) => json!({ "url": url, "error": error }),
    }
}

async fn fetch_readiness(config: &Config, url: &str) -> Result<(u16, Value), String> {
    tls::install_provider();
    let cert_path = certificate_path(config);
    let pem = std::fs::read(&cert_path)
        .map_err(|error| format!("{} cannot be read: {error}", cert_path.display()))?;
    let certificate = reqwest::Certificate::from_pem(&pem)
        .map_err(|error| format!("{} is not a PEM certificate: {error}", cert_path.display()))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([certificate])
        .timeout(READINESS_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|error| error.to_string())?;
    Ok((
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readiness_address_replaces_an_unspecified_ip_with_loopback() {
        assert_eq!(
            readiness_address("0.0.0.0:8443".parse().unwrap()),
            "127.0.0.1:8443".parse().unwrap()
        );
        assert_eq!(
            readiness_address("[::]:8443".parse().unwrap()),
            "[::1]:8443".parse().unwrap()
        );
        assert_eq!(
            readiness_address("10.0.0.5:1500".parse().unwrap()),
            "10.0.0.5:1500".parse().unwrap()
        );
    }

    #[test]
    fn the_policy_and_certificate_paths_follow_the_configuration() {
        let mut config = Config::parse(None, |_| None).unwrap();
        config.file = PathBuf::from("/etc/keel/config.yaml");
        config.data_dir = PathBuf::from("/var/lib/keel");
        assert_eq!(policy_path(&config), PathBuf::from("/etc/keel/agent.yaml"));
        assert_eq!(
            certificate_path(&config),
            PathBuf::from("/var/lib/keel/tls/cert.pem")
        );
        config.tls.cert = "/etc/keel/own.crt".into();
        assert_eq!(
            certificate_path(&config),
            PathBuf::from("/etc/keel/own.crt")
        );
        config.file = PathBuf::from("config.yaml");
        assert_eq!(policy_path(&config), PathBuf::from(POLICY_FILE));
    }
}
