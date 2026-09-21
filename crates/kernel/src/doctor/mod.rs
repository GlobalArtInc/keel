mod certificate;
mod database;
mod files;
mod units;

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use keel_sdk::KeelError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

use crate::clock::Clock;
use crate::config::Tls;
use crate::loader::Graph;
use crate::migrations;

pub use database::{Database, Ledger, LedgerStatus, ledger_status};
pub(crate) use database::{statement, table_exists};

pub const OPT: &str = "/opt/keel";
pub const UNIT_DIR: &str = "/etc/systemd/system";
pub const SYSTEMCTL: &str = "systemctl";
pub const AGENT_SOCKET: &str = "/run/keel/agent.sock";
const HOST_MODULE: &str = "host";

pub(crate) const DATA_DIR: &str = "data_dir";
pub(crate) const CONFIG_DIR: &str = "config_dir";
pub(crate) const INSTALL_DIR: &str = "install_dir";
pub(crate) const SECRET_KEY: &str = "secret_key";
pub(crate) const TLS: &str = "tls";
pub(crate) const UNITS: &str = "units";
pub(crate) const DATABASE: &str = "database";
pub(crate) const MIGRATIONS: &str = "migrations";
pub(crate) const AGENT: &str = "agent";
pub(crate) const LICENSE: &str = "license";
pub(crate) const SPACE: &str = "space";
pub(crate) const PRODUCTS: &str = "products";
pub(crate) const ORPHANS: &str = "orphans";
pub(crate) const ACCOUNTS: &str = "accounts";
pub const CHECKS: [&str; 14] = [
    DATA_DIR,
    CONFIG_DIR,
    INSTALL_DIR,
    SECRET_KEY,
    TLS,
    UNITS,
    DATABASE,
    MIGRATIONS,
    AGENT,
    LICENSE,
    SPACE,
    PRODUCTS,
    ORPHANS,
    ACCOUNTS,
];

const ERROR_NOT_STARTED: &str = "keel.error.doctor_not_started";
const AGENT_TIMEOUT: Duration = Duration::from_secs(3);
const LICENSE_STUB: &str = "not required; the license module arrives with M5";

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Check {
    pub id: String,
    pub status: Status,
    pub message: String,
}

impl Check {
    pub(crate) fn ok(id: &str, message: impl Into<String>) -> Self {
        Self::new(id, Status::Ok, message)
    }

    pub(crate) fn warn(id: &str, message: impl Into<String>) -> Self {
        Self::new(id, Status::Warn, message)
    }

    pub(crate) fn fail(id: &str, message: impl Into<String>) -> Self {
        Self::new(id, Status::Fail, message)
    }

    fn new(id: &str, status: Status, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            status,
            message: message.into(),
        }
    }

    pub fn line(&self) -> String {
        format!("{:<4}  {:<12} {}", self.status, self.id, self.message)
    }
}

pub(super) struct Findings {
    id: &'static str,
    problems: Vec<(Status, String)>,
}

impl Findings {
    pub(super) fn new(id: &'static str) -> Self {
        Self {
            id,
            problems: Vec::new(),
        }
    }

    pub(super) fn fail(&mut self, message: impl Into<String>) {
        self.problems.push((Status::Fail, message.into()));
    }

    pub(super) fn warn(&mut self, message: impl Into<String>) {
        self.problems.push((Status::Warn, message.into()));
    }

    pub(super) fn finish(self, healthy: impl Into<String>) -> Check {
        let status = self
            .problems
            .iter()
            .map(|(status, _)| *status)
            .max()
            .unwrap_or(Status::Ok);
        let message = if self.problems.is_empty() {
            healthy.into()
        } else {
            self.problems
                .into_iter()
                .map(|(_, message)| message)
                .collect::<Vec<_>>()
                .join("; ")
        };
        Check::new(self.id, status, message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Report {
    pub at: DateTime<Utc>,
    pub version: String,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.checks.iter().any(|check| check.status == Status::Fail)
    }

    fn count(&self, status: Status) -> usize {
        self.checks
            .iter()
            .filter(|check| check.status == status)
            .count()
    }

    pub fn check(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.id == id)
    }

    pub fn render(&self) -> String {
        let mut text = String::new();
        for check in &self.checks {
            text.push_str(&check.line());
            text.push('\n');
        }
        text.push_str(&format!(
            "{} ok, {} warn, {} fail; keel {} at {}\n",
            self.count(Status::Ok),
            self.count(Status::Warn),
            self.count(Status::Fail),
            self.version,
            self.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ));
        text
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub opt: PathBuf,
    pub unit_dir: PathBuf,
    pub systemctl: PathBuf,
    pub agent_socket: PathBuf,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            opt: PathBuf::from(OPT),
            unit_dir: PathBuf::from(UNIT_DIR),
            systemctl: PathBuf::from(SYSTEMCTL),
            agent_socket: PathBuf::from(AGENT_SOCKET),
        }
    }
}

pub struct Doctor {
    version: String,
    data_dir: PathBuf,
    config_file: PathBuf,
    tls: Tls,
    carried: Vec<String>,
    ledgers: Vec<Ledger>,
    host_in_use: bool,
    layout: Layout,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for Doctor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Doctor")
            .field("version", &self.version)
            .field("data_dir", &self.data_dir)
            .field("config_file", &self.config_file)
            .field("carried", &self.carried)
            .field("ledgers", &self.ledgers.len())
            .field("host_in_use", &self.host_in_use)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

fn ledgers_of(graph: &Graph) -> Vec<Ledger> {
    let mut ledgers = vec![Ledger {
        module: crate::keel::MODULE.id.to_owned(),
        table: migrations::LEDGER.to_owned(),
        catalogue: &migrations::KERNEL,
    }];
    for def in graph.modules() {
        if let Some(catalogue) = def.migrations {
            ledgers.push(Ledger {
                module: def.id.to_owned(),
                table: catalogue.ledger().to_owned(),
                catalogue,
            });
        }
    }
    ledgers
}

impl Doctor {
    pub fn new(graph: &Graph, tls: &Tls, layout: Layout) -> Self {
        Self {
            version: graph.version().to_owned(),
            data_dir: graph.data_dir().to_owned(),
            config_file: graph.sections().file().to_owned(),
            tls: tls.clone(),
            carried: graph
                .manifests()
                .iter()
                .map(|bundle| bundle.id.to_owned())
                .collect(),
            ledgers: ledgers_of(graph),
            host_in_use: graph.ids().contains(&HOST_MODULE),
            layout,
            clock: graph.clock().clone(),
        }
    }

    pub fn ledgers(&self) -> &[Ledger] {
        &self.ledgers
    }

    pub async fn run(&self, database: &Database<'_>) -> Report {
        let now = self.clock.now();
        let checks = vec![
            files::data_dir(&self.data_dir),
            files::config_dir(&self.config_file),
            files::install_dir(&self.layout.opt, &self.version),
            files::secret_key(&self.data_dir),
            certificate::check(&self.tls, &self.data_dir, now),
            units::check(&self.layout).await,
            database::server(database).await,
            database::migrations(database, &self.ledgers).await,
            self.agent().await,
            Check::ok(LICENSE, LICENSE_STUB),
            files::space(&self.data_dir, &self.layout.opt),
            database::products(database, &self.carried).await,
            database::orphans(database).await,
            database::accounts(database).await,
        ];
        Report {
            at: now,
            version: self.version.clone(),
            checks,
        }
    }

    async fn agent(&self) -> Check {
        let socket = &self.layout.agent_socket;
        if !self.host_in_use {
            return Check::ok(
                AGENT,
                format!(
                    "not in use; no enabled product loads the {HOST_MODULE} module, so {} is never opened",
                    socket.display()
                ),
            );
        }
        match tokio::time::timeout(AGENT_TIMEOUT, UnixStream::connect(socket)).await {
            Ok(Ok(_)) => Check::ok(
                AGENT,
                format!("in use and reachable at {}", socket.display()),
            ),
            Ok(Err(error)) => Check::fail(
                AGENT,
                format!(
                    "in use but {} does not answer: {error}; is keel-privileged.service running?",
                    socket.display()
                ),
            ),
            Err(_) => Check::fail(
                AGENT,
                format!(
                    "in use but {} did not accept a connection within {}s",
                    socket.display(),
                    AGENT_TIMEOUT.as_secs()
                ),
            ),
        }
    }
}

#[derive(Default)]
pub struct Handle {
    attached: OnceLock<Arc<Doctor>>,
}

impl Handle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attach(&self, doctor: Arc<Doctor>) -> bool {
        self.attached.set(doctor).is_ok()
    }

    pub fn doctor(&self) -> Result<Arc<Doctor>, KeelError> {
        self.attached
            .get()
            .cloned()
            .ok_or_else(|| KeelError::unavailable(ERROR_NOT_STARTED))
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handle")
            .field("attached", &self.attached.get().is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_prints_as_one_line_and_findings_take_the_worst_status() {
        let ok = Check::ok(DATA_DIR, "/var/lib/keel is owned by keel with mode 0700");
        assert_eq!(
            ok.line(),
            "ok    data_dir     /var/lib/keel is owned by keel with mode 0700"
        );
        let mut findings = Findings::new(UNITS);
        findings.warn("keel.service is inactive");
        findings.fail("keel-update.path is missing");
        let check = findings.finish("all good");
        assert_eq!(check.status, Status::Fail);
        assert_eq!(
            check.message,
            "keel.service is inactive; keel-update.path is missing"
        );
        assert_eq!(
            check.line(),
            "fail  units        keel.service is inactive; keel-update.path is missing"
        );
        let clean = Findings::new(UNITS).finish("all good");
        assert_eq!(clean.status, Status::Ok);
        assert_eq!(clean.message, "all good");
        let mut warned = Findings::new(SPACE);
        warned.warn("low");
        assert_eq!(warned.finish("x").status, Status::Warn);
        assert!(Status::Fail > Status::Warn && Status::Warn > Status::Ok);
        assert_eq!(Status::Warn.to_string(), "warn");
    }

    #[test]
    fn a_report_renders_its_lines_and_its_totals_and_knows_when_it_failed() {
        let at: DateTime<Utc> = "2026-09-21T10:15:30Z".parse().unwrap();
        let report = Report {
            at,
            version: "0.1.0".into(),
            checks: vec![
                Check::ok(LICENSE, LICENSE_STUB),
                Check::warn(ORPHANS, "note is orphaned"),
            ],
        };
        assert!(!report.failed());
        assert_eq!(report.count(Status::Ok), 1);
        assert_eq!(report.count(Status::Warn), 1);
        assert_eq!(
            report.check(ORPHANS).map(|check| check.status),
            Some(Status::Warn)
        );
        assert!(report.check(UNITS).is_none());
        let text = report.render();
        assert!(
            text.starts_with("ok    license      not required"),
            "{text}"
        );
        assert!(
            text.ends_with("1 ok, 1 warn, 0 fail; keel 0.1.0 at 2026-09-21T10:15:30Z\n"),
            "{text}"
        );
        let failed = Report {
            checks: vec![Check::fail(UNITS, "missing")],
            ..report
        };
        assert!(failed.failed());
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["checks"][0]["status"], "fail");
        assert_eq!(json["checks"][0]["id"], "units");
    }

    #[test]
    fn the_layout_defaults_to_the_paths_of_the_design_and_the_handle_starts_detached() {
        let layout = Layout::default();
        assert_eq!(layout.opt, PathBuf::from("/opt/keel"));
        assert_eq!(layout.unit_dir, PathBuf::from("/etc/systemd/system"));
        assert_eq!(layout.systemctl, PathBuf::from("systemctl"));
        assert_eq!(layout.agent_socket, PathBuf::from("/run/keel/agent.sock"));
        let handle = Handle::new();
        assert_eq!(format!("{handle:?}"), "Handle { attached: false }");
        let error = handle.doctor().unwrap_err();
        assert_eq!(error.key(), ERROR_NOT_STARTED);
        assert_eq!(CHECKS.len(), 14);
        let unique: std::collections::BTreeSet<&str> = CHECKS.iter().copied().collect();
        assert_eq!(unique.len(), CHECKS.len());
    }
}
