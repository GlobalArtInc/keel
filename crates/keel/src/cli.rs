use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use keel_host_agent::exec::{self, Target};
use keel_host_agent::home;
use keel_host_agent::{DEFAULT_TEMPLATES, DEFAULT_USER, Policy};
use keel_kernel::ctl::{self, Out};
use keel_kernel::doctor::{self, Layout};
use keel_kernel::{Config, logging, restart};
use keel_release::{KeyRing, PublicKey};
use keel_types::TaskState;
use serde_json::{Value, json};
use tokio::signal::unix::{SignalKind, signal};

use crate::VERSION;
use crate::backup::{self, BackupCliError, RESTORE_CLI, describe_manifest};
use crate::doctor::SupportCliError;
use crate::error::{Error, describe};
use crate::license::{self, INSTALL as LICENSE_INSTALL};
use crate::privileged::{self, HostPolicy, PrivilegedError, Request, Runner, Source};
use crate::products::{Switched, Wanted};
use crate::runtime::{Runtime, Stopped};

const UPDATE_CHECK: &str = "update.check";
const UPDATE_APPLY: &str = "update.apply";
const UPDATE_ROLLBACK: &str = "update.rollback";
const TASK_GET: &str = "task.get";
const TASK_POLL: Duration = Duration::from_millis(500);
const EXIT_CANNOT_EXECUTE: u8 = 126;

#[derive(Debug, Parser)]
#[command(name = "keel", version = VERSION, about = "The keel installation: serve, migrate, product, ctl, update, backup, restore, doctor, support-bundle and privileged")]
pub(crate) struct Cli {
    #[arg(long, global = true, env = "KEEL_CONFIG", value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    #[command(about = "Run the kernel: the HTTP listener, the control socket and the modules")]
    Serve,
    #[command(about = "Apply the migrations of the enabled composition and exit")]
    Migrate,
    #[command(
        about = "List product bundles and their license features, enable or disable a bundle"
    )]
    Product {
        #[command(subcommand)]
        action: ProductAction,
    },
    #[command(about = "Call a function over the control socket")]
    Ctl(CtlArgs),
    #[command(
        about = "Check the release channel, apply the available version or roll back, over the control socket"
    )]
    Update(UpdateCommand),
    #[command(
        about = "The one root code path of the installation: serves the update policy, and the host policy once a product loads the host module"
    )]
    Privileged(PrivilegedArgs),
    #[command(about = "Install a signed license lease over the control socket")]
    License(LicenseCommand),
    #[command(about = "Write a backup archive of the installation, or inspect the manifest of one")]
    Backup(BackupArgs),
    #[command(about = "Restore a backup archive while the service is stopped")]
    Restore(RestoreArgs),
    #[command(
        about = "Check the installation end to end and print one line per check; exit 1 when a check fails"
    )]
    Doctor(DoctorArgs),
    #[command(
        about = "Collect the redacted configuration, the log ring, the ledgers, the audit tail, the open problems and the doctor report into an archive"
    )]
    SupportBundle(SupportArgs),
    #[command(
        hide = true,
        about = "Run a program under a uid of the installation's range; the agent re-executes itself through this subcommand instead of changing its own credentials"
    )]
    AgentExec(AgentExecArgs),
    #[command(
        hide = true,
        about = "Run one file operation inside a home as the account agent-exec became; the agent's operations on a home re-execute the binary through this subcommand"
    )]
    AgentHome(AgentHomeArgs),
}

#[derive(Debug, Args)]
pub(crate) struct AgentHomeArgs {
    #[arg(
        value_name = "ARG",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(crate) struct AgentExecArgs {
    #[arg(long, value_name = "UID")]
    pub(crate) uid: u32,
    #[arg(long, default_value = privileged::DEFAULT_POLICY, value_name = "PATH")]
    pub(crate) policy: PathBuf,
    #[arg(value_name = "PROGRAM", required = true)]
    pub(crate) program: PathBuf,
    #[arg(
        value_name = "ARG",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) args: Vec<OsString>,
}

#[derive(Debug, Args)]
pub(crate) struct LayoutArgs {
    #[arg(long, default_value = doctor::OPT, value_name = "DIR")]
    pub(crate) opt: PathBuf,
    #[arg(long, default_value = doctor::UNIT_DIR, value_name = "DIR")]
    pub(crate) unit_dir: PathBuf,
    #[arg(long, default_value = doctor::SYSTEMCTL, value_name = "PATH")]
    pub(crate) systemctl: PathBuf,
    #[arg(
        long,
        value_name = "PATH",
        help = "The agent socket; the socket of the host section of the configuration, else /run/keel/agent.sock"
    )]
    pub(crate) agent_socket: Option<PathBuf>,
}

impl From<LayoutArgs> for Layout {
    fn from(args: LayoutArgs) -> Self {
        Self {
            opt: args.opt,
            unit_dir: args.unit_dir,
            systemctl: args.systemctl,
            agent_socket: args
                .agent_socket
                .unwrap_or_else(|| PathBuf::from(doctor::AGENT_SOCKET)),
        }
    }
}

impl LayoutArgs {
    fn of_site(mut self, config: &Config) -> Layout {
        if self.agent_socket.is_none() {
            self.agent_socket = crate::doctor::agent_socket(config);
        }
        Layout::from(self)
    }
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[command(flatten)]
    pub(crate) layout: LayoutArgs,
    #[arg(long, value_enum, default_value_t = ReportOut::Text)]
    pub(crate) out: ReportOut,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReportOut {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Args)]
pub(crate) struct SupportArgs {
    #[command(flatten)]
    pub(crate) layout: LayoutArgs,
    #[arg(
        long,
        value_name = "PATH",
        help = "Where to write the archive; the default is <data_dir>/support/keel-support-<version>-<instant>.tar.zst"
    )]
    pub(crate) out: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct BackupArgs {
    #[arg(
        long,
        value_name = "PATH",
        help = "Where to write the archive; the default is <data_dir>/backups/keel-<version>-<instant>.tar.zst"
    )]
    pub(crate) out: Option<PathBuf>,
    #[arg(
        long,
        help = "Include secret.key, sealed under a passphrase read from KEEL_BACKUP_PASSPHRASE or the terminal"
    )]
    pub(crate) with_secret_key: bool,
    #[arg(long, env = keel_kernel::backup::PASSPHRASE_VARIABLE, hide = true, hide_env_values = true)]
    pub(crate) passphrase: Option<String>,
    #[command(subcommand)]
    pub(crate) action: Option<BackupAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BackupAction {
    #[command(about = "Print the manifest of an archive as JSON")]
    Inspect {
        #[arg(value_name = "ARCHIVE")]
        archive: PathBuf,
    },
}

#[derive(Debug, Args)]
pub(crate) struct RestoreArgs {
    #[arg(value_name = "ARCHIVE")]
    pub(crate) archive: PathBuf,
    #[arg(
        long,
        help = "Restore secret.key from the archive, opened with the passphrase from KEEL_BACKUP_PASSPHRASE or the terminal"
    )]
    pub(crate) with_secret_key: bool,
    #[arg(long, env = keel_kernel::backup::PASSPHRASE_VARIABLE, hide = true, hide_env_values = true)]
    pub(crate) passphrase: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct PrivilegedArgs {
    #[arg(long, default_value = privileged::DEFAULT_SOCKET, value_name = "PATH")]
    pub(crate) socket: PathBuf,
    #[arg(long, default_value = privileged::DEFAULT_POLICY, value_name = "PATH")]
    pub(crate) policy: PathBuf,
    #[arg(
        long,
        default_value = DEFAULT_USER,
        value_name = "NAME",
        help = "The one account whose connections the agent socket admits, checked by peer credentials"
    )]
    pub(crate) user: String,
    #[arg(
        long,
        default_value = DEFAULT_TEMPLATES,
        value_name = "DIR",
        help = "The directory the operator's template overrides are read from"
    )]
    pub(crate) templates: PathBuf,
    #[command(subcommand)]
    pub(crate) action: Option<PrivilegedAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PrivilegedAction {
    #[command(
        about = "Install the signed version the update request names, or roll back to the previous version"
    )]
    Update(UpdateArgs),
}

#[derive(Debug, Args)]
#[command(subcommand_negates_reqs = true)]
pub(crate) struct UpdateArgs {
    #[arg(
        long,
        required = true,
        help = "Read the version, the channel and the principal from the request file and install that version"
    )]
    pub(crate) from_request: bool,
    #[arg(long, default_value = privileged::DEFAULT_REQUEST, value_name = "PATH")]
    pub(crate) request: PathBuf,
    #[arg(long, default_value = keel_release::CHANNEL_URL, value_name = "URL|DIR")]
    pub(crate) channel_url: String,
    #[arg(long, default_value = privileged::DEFAULT_OPT, value_name = "DIR")]
    pub(crate) opt: PathBuf,
    #[arg(long, default_value = privileged::DEFAULT_UNIT_DIR, value_name = "DIR")]
    pub(crate) unit_dir: PathBuf,
    #[arg(long, default_value = privileged::DEFAULT_SYSTEMCTL, value_name = "PATH")]
    pub(crate) systemctl: PathBuf,
    #[arg(
        long,
        value_name = "PUBLIC_KEY",
        help = "Verify the manifests against this key instead of the release keys compiled in; for rehearsals"
    )]
    pub(crate) release_key: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) action: Option<UpdateAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum UpdateAction {
    #[command(
        about = "Switch bin/current back to the previous version; across a contract boundary only with --restore"
    )]
    Rollback {
        #[arg(
            long,
            value_name = "ARCHIVE",
            help = "The pre-update backup to restore across a contract boundary; keel.service is stopped instead of restarted"
        )]
        restore: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct UpdateCommand {
    #[command(subcommand)]
    pub(crate) step: UpdateStep,
    #[arg(long, global = true, value_enum, default_value_t = OutArg::Json)]
    pub(crate) out: OutArg,
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) socket: Option<PathBuf>,
    #[arg(long, global = true, default_value = "en", value_name = "CODE")]
    pub(crate) lang: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum UpdateStep {
    #[command(about = "Compare the running version with the head of the release channel")]
    Check,
    #[command(
        about = "Drain the tasks, dump the database and hand the available version to the root unit"
    )]
    Apply {
        #[arg(
            long,
            help = "Follow the task until it ends and print the job instead of the task reference"
        )]
        wait: bool,
    },
    #[command(about = "The same for the version this installation was updated from")]
    Rollback {
        #[arg(
            long,
            help = "Follow the task until it ends and print the job instead of the task reference"
        )]
        wait: bool,
    },
}

impl UpdateStep {
    fn function(&self) -> &'static str {
        match self {
            UpdateStep::Check => UPDATE_CHECK,
            UpdateStep::Apply { .. } => UPDATE_APPLY,
            UpdateStep::Rollback { .. } => UPDATE_ROLLBACK,
        }
    }

    fn waits(&self) -> bool {
        matches!(
            self,
            UpdateStep::Apply { wait: true } | UpdateStep::Rollback { wait: true }
        )
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProductAction {
    #[command(about = "Print every bundle this binary carries with its state")]
    List,
    #[command(about = "Print the license features of the enabled bundles, one per line")]
    Features,
    #[command(
        about = "Enable a bundle: over the control socket while the service runs, which restarts it, or in the database while it is stopped"
    )]
    Enable { id: String },
    #[command(
        about = "Disable a bundle the same way; its modules become orphaned and their tables stay in place"
    )]
    Disable {
        id: String,
        #[arg(
            long,
            help = "Disable a product another enabled bundle reaches through a local bridge; that bundle moves to its remote side, which answers unavailable until a neighbour is paired"
        )]
        switch_remote: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct LicenseCommand {
    #[command(subcommand)]
    pub(crate) action: LicenseAction,
    #[arg(long, global = true, value_enum, default_value_t = OutArg::Json)]
    pub(crate) out: OutArg,
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) socket: Option<PathBuf>,
    #[arg(long, global = true, default_value = "en", value_name = "CODE")]
    pub(crate) lang: String,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LicenseAction {
    #[command(
        about = "Verify a signed lease against the license key and store it; the answer is the new license state"
    )]
    Install {
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
}

#[derive(Debug, Args)]
pub(crate) struct CtlArgs {
    #[arg(value_name = "FUNC")]
    pub(crate) func: String,
    #[arg(value_name = "KEY=VALUE")]
    pub(crate) pairs: Vec<String>,
    #[arg(long, value_enum, default_value_t = OutArg::Json)]
    pub(crate) out: OutArg,
    #[arg(long, value_name = "PATH")]
    pub(crate) socket: Option<PathBuf>,
    #[arg(long, default_value = "en", value_name = "CODE")]
    pub(crate) lang: String,
    #[arg(long = "as", value_name = "LOGIN")]
    pub(crate) run_as: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutArg {
    #[default]
    Json,
    Table,
}

impl From<OutArg> for Out {
    fn from(out: OutArg) -> Self {
        match out {
            OutArg::Json => Out::Json,
            OutArg::Table => Out::Table,
        }
    }
}

impl Runtime {
    pub fn run(self) -> ExitCode {
        self.run_from(std::env::args_os())
    }

    pub fn run_from<I, T>(self, args: I) -> ExitCode
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let cli = match Cli::try_parse_from(args) {
            Ok(cli) => cli,
            Err(error) => {
                let _ = error.print();
                return ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2));
            }
        };
        match self.execute(cli) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("error: {}", describe(&error));
                ExitCode::FAILURE
            }
        }
    }

    fn execute(&self, cli: Cli) -> Result<ExitCode, Error> {
        let command = match cli.command {
            Command::AgentExec(args) => return Ok(agent_exec(args)),
            Command::AgentHome(args) => return Ok(ExitCode::from(home::serve(&args.args))),
            command => command,
        };
        let executor = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::Executor)?;
        executor.block_on(async {
            match command {
                Command::Serve => {
                    let config = load(cli.config.as_deref())?;
                    logging::init(&config.log)?;
                    Ok(match self.serve_until(&config, signals()).await? {
                        Stopped::Shutdown => ExitCode::SUCCESS,
                        Stopped::Restart(_) => ExitCode::from(restart::EXIT_STATUS),
                    })
                }
                Command::Migrate => {
                    let config = load(cli.config.as_deref())?;
                    let migrated = self.migrate(&config).await?;
                    println!("kernel: {}", applied(&migrated.kernel));
                    for (module, names) in &migrated.modules {
                        println!("{module}: {}", applied(names));
                    }
                    if !migrated.orphaned.is_empty() {
                        println!("orphaned: {}", migrated.orphaned.join(", "));
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Command::Product { action } => {
                    let config = load(cli.config.as_deref())?;
                    let (id, wanted, switch_remote) = match action {
                        ProductAction::List => {
                            for state in self.list_products(&config).await? {
                                println!("{state}");
                            }
                            return Ok(ExitCode::SUCCESS);
                        }
                        ProductAction::Features => {
                            for feature in self.product_features(&config).await? {
                                println!("{feature}");
                            }
                            return Ok(ExitCode::SUCCESS);
                        }
                        ProductAction::Enable { id } => (id, Wanted::Enabled, false),
                        ProductAction::Disable { id, switch_remote } => {
                            (id, Wanted::Disabled, switch_remote)
                        }
                    };
                    Ok(
                        match self
                            .switch_product(&config, &id, wanted, switch_remote)
                            .await?
                        {
                            Switched::Refused(text) => {
                                eprintln!("{}", text.trim_end_matches('\n'));
                                ExitCode::FAILURE
                            }
                            switched => {
                                println!("{id}: {switched}");
                                ExitCode::SUCCESS
                            }
                        },
                    )
                }
                Command::Privileged(args) => match args.action {
                    None => {
                        let host = HostPolicy {
                            socket: &args.socket,
                            policy: &args.policy,
                            user: &args.user,
                            templates: &args.templates,
                            catalogue: self.host_catalogue()?,
                        };
                        Ok(match privileged::serve(host, signals()).await {
                            Ok(()) => ExitCode::SUCCESS,
                            Err(error) => {
                                eprintln!("error: {}", describe(&error));
                                ExitCode::FAILURE
                            }
                        })
                    }
                    Some(PrivilegedAction::Update(update)) => Ok(privileged_update(update).await),
                },
                Command::AgentExec(args) => Ok(agent_exec(args)),
                Command::AgentHome(args) => Ok(ExitCode::from(home::serve(&args.args))),
                Command::Ctl(args) => {
                    let socket = match args.socket {
                        Some(socket) => socket,
                        None => load(cli.config.as_deref())?.socket,
                    };
                    let request = ctl::Request {
                        func: args.func,
                        params: ctl::params(args.pairs.iter().map(String::as_str))?,
                        out: args.out.into(),
                        lang: args.lang,
                        run_as: args.run_as,
                    };
                    Ok(print(ctl::run(&socket, &request).await?))
                }
                Command::Update(args) => {
                    let socket = match &args.socket {
                        Some(socket) => socket.clone(),
                        None => load(cli.config.as_deref())?.socket,
                    };
                    Ok(update(&socket, &args).await)
                }
                Command::License(args) => {
                    let socket = match &args.socket {
                        Some(socket) => socket.clone(),
                        None => load(cli.config.as_deref())?.socket,
                    };
                    let LicenseAction::Install { file } = &args.action;
                    let lease = match license::read_lease(file) {
                        Ok(lease) => lease,
                        Err(error) => {
                            eprintln!("error: {error}");
                            return Ok(ExitCode::FAILURE);
                        }
                    };
                    let request = ctl::Request {
                        func: LICENSE_INSTALL.to_owned(),
                        params: json!({ "lease": lease }),
                        out: args.out.into(),
                        lang: args.lang.clone(),
                        run_as: None,
                    };
                    Ok(print(ctl::run(&socket, &request).await?))
                }
                Command::Backup(args) => {
                    let config = load(cli.config.as_deref())?;
                    Ok(report(self.backup_command(&config, args).await))
                }
                Command::Restore(args) => {
                    let config = load(cli.config.as_deref())?;
                    Ok(report(self.restore_command(&config, args).await))
                }
                Command::Doctor(args) => {
                    let config = load(cli.config.as_deref())?;
                    let report = self.doctor(&config, args.layout.of_site(&config)).await?;
                    match args.out {
                        ReportOut::Text => print!("{}", report.render()),
                        ReportOut::Json => println!(
                            "{}",
                            serde_json::to_string_pretty(&report)
                                .unwrap_or_else(|_| report.render())
                        ),
                    }
                    Ok(if report.failed() {
                        ExitCode::FAILURE
                    } else {
                        ExitCode::SUCCESS
                    })
                }
                Command::SupportBundle(args) => {
                    let config = load(cli.config.as_deref())?;
                    Ok(support(
                        self.support_bundle(
                            &config,
                            args.layout.of_site(&config),
                            args.out.as_deref(),
                        )
                        .await,
                    ))
                }
            }
        })
    }

    async fn backup_command(
        &self,
        config: &Config,
        args: BackupArgs,
    ) -> Result<(), BackupCliError> {
        if let Some(BackupAction::Inspect { archive }) = args.action {
            let manifest =
                tokio::task::spawn_blocking(move || keel_kernel::backup::read_manifest(&archive))
                    .await
                    .map_err(|error| BackupCliError::Terminal(std::io::Error::other(error)))??;
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest)
                    .unwrap_or_else(|_| describe_manifest(&manifest))
            );
            return Ok(());
        }
        let passphrase = if args.with_secret_key {
            Some(backup::passphrase("seal", args.passphrase)?)
        } else {
            None
        };
        let created = self
            .backup(config, args.out.as_deref(), passphrase.as_ref())
            .await?;
        println!(
            "wrote {} ({} bytes): {}",
            created.archive.display(),
            created.bytes,
            describe_manifest(&created.manifest)
        );
        Ok(())
    }

    async fn restore_command(
        &self,
        config: &Config,
        args: RestoreArgs,
    ) -> Result<(), BackupCliError> {
        let passphrase = if args.with_secret_key {
            Some(backup::passphrase("open", args.passphrase)?)
        } else {
            None
        };
        let restored = self
            .restore(config, &args.archive, passphrase.as_ref(), RESTORE_CLI)
            .await?;
        let note = &restored.note;
        println!(
            "restored {} (keel {}) into {}: {} table(s) loaded, {} file(s) written",
            note.archive.display(),
            note.keel,
            config.data_dir.display(),
            note.tables,
            restored.files_written
        );
        if !restored.skipped_tables.is_empty() {
            println!(
                "skipped {} table(s) without a table in this schema: {}",
                restored.skipped_tables.len(),
                restored.skipped_tables.join(", ")
            );
        }
        println!(
            "enabled products: {} -> {} ({})",
            list_or_none(&note.products_before),
            list_or_none(&note.products_after),
            note.changes()
        );
        match (&restored.secret_key, note.data_key.as_str()) {
            (Some(path), _) => println!("secret key: written to {}", path.display()),
            (None, "kept") => {
                println!("secret key: the key of this host opens the data key of the archive")
            }
            (None, "dropped") => println!(
                "secret key: the archive carries none and the key of this host does not open its data key; a new data key is generated at the next start and the sealed values of the dump will not open"
            ),
            (None, _) => println!("secret key: the archive carries no wrapped data key"),
        }
        for kept in &restored.config_kept {
            println!(
                "configuration: {kept} of this host is kept; the archive's copy is beside it as {kept}{}",
                keel_kernel::backup::KEPT_SUFFIX
            );
        }
        if let Some(fallback) = &restored.config_fallback {
            println!(
                "configuration: {} is not writable by this user; the archive's tree is at {}: copy what belongs there as root",
                config
                    .file
                    .parent()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_default(),
                fallback.display()
            );
        }
        if !note.sources.is_empty() {
            println!("sources restored: {}", note.sources.join(", "));
        }
        if !restored.sources_skipped.is_empty() {
            println!(
                "sources left in the archive (no module restores them): {}",
                restored.sources_skipped.join(", ")
            );
        }
        println!("start the service to run on the restored installation");
        Ok(())
    }
}

fn support(outcome: Result<keel_kernel::support::Written, SupportCliError>) -> ExitCode {
    match outcome {
        Ok(written) => {
            println!(
                "wrote {} ({} bytes): {}",
                written.archive.display(),
                written.bytes,
                written.entries.join(", ")
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

fn report(outcome: Result<(), BackupCliError>) -> ExitCode {
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

fn list_or_none(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".to_owned()
    } else {
        ids.join(", ")
    }
}

fn print(output: ctl::Output) -> ExitCode {
    if output.ok {
        println!("{}", output.text.trim_end_matches('\n'));
        ExitCode::SUCCESS
    } else {
        eprintln!("{}", output.text.trim_end_matches('\n'));
        ExitCode::FAILURE
    }
}

#[derive(Debug, thiserror::Error)]
enum UpdateCliError {
    #[error(transparent)]
    Ctl(#[from] ctl::CtlError),
    #[error("{function} answered something that is not the expected envelope: {answer}")]
    Answer { function: String, answer: String },
}

async fn update(socket: &Path, args: &UpdateCommand) -> ExitCode {
    match follow_update(socket, args).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

async fn follow_update(socket: &Path, args: &UpdateCommand) -> Result<ExitCode, UpdateCliError> {
    let request = |func: &str, params: Value, out: Out| ctl::Request {
        func: func.to_owned(),
        params,
        out,
        lang: args.lang.clone(),
        run_as: None,
    };
    let func = args.step.function();
    if !args.step.waits() {
        return Ok(print(
            ctl::run(socket, &request(func, json!({}), args.out.into())).await?,
        ));
    }
    let started = ctl::run(socket, &request(func, json!({}), Out::Json)).await?;
    if !started.ok {
        return Ok(print(started));
    }
    let id = serde_json::from_str::<Value>(&started.text)
        .ok()
        .and_then(|envelope| envelope["data"]["task"]["id"].as_str().map(str::to_owned))
        .ok_or_else(|| UpdateCliError::Answer {
            function: func.to_owned(),
            answer: started.text.clone(),
        })?;
    loop {
        let job = ctl::run(socket, &request(TASK_GET, json!({ "id": id }), Out::Json)).await?;
        if !job.ok {
            return Ok(print(job));
        }
        let state = serde_json::from_str::<Value>(&job.text)
            .ok()
            .and_then(|envelope| {
                serde_json::from_value::<TaskState>(envelope["data"]["state"].clone()).ok()
            })
            .ok_or_else(|| UpdateCliError::Answer {
                function: TASK_GET.to_owned(),
                answer: job.text.clone(),
            })?;
        if state.is_final() {
            let shown = ctl::run(
                socket,
                &request(TASK_GET, json!({ "id": id }), args.out.into()),
            )
            .await?;
            let code = print(shown);
            return Ok(if state == TaskState::Done {
                code
            } else {
                ExitCode::FAILURE
            });
        }
        tokio::time::sleep(TASK_POLL).await;
    }
}

async fn privileged_update(args: UpdateArgs) -> ExitCode {
    if args.from_request && args.action.is_some() {
        eprintln!("error: update takes --from-request or rollback, not both");
        return ExitCode::from(2);
    }
    match run_update(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}

async fn run_update(args: UpdateArgs) -> Result<(), PrivilegedError> {
    let keys = match &args.release_key {
        Some(path) => KeyRing::new(PublicKey::read(path)?, Vec::new()),
        None => KeyRing::compiled()?,
    };
    let runner = Runner {
        opt: args.opt,
        unit_dir: args.unit_dir,
        systemctl: args.systemctl,
        keys,
        source: Source::parse(&args.channel_url)?,
    };
    match args.action {
        Some(UpdateAction::Rollback { restore }) => {
            let switched = runner.rollback(restore.as_deref()).await?;
            println!(
                "rolled back to {}; previous is now {}{}",
                switched.version,
                switched.previous,
                units_line(&switched.units)
            );
            if let Some(archive) = switched.restore {
                println!(
                    "keel.service is stopped: restore {} as the user keel with \"keel restore\", then systemctl start keel.service",
                    archive.display()
                );
            }
        }
        None => {
            let request = Request::consume(&args.request)?;
            let installed = runner.install(&request).await?;
            println!(
                "installed {} from {} requested by {}; previous {}{}",
                installed.version,
                installed.channel,
                installed.requested_by,
                installed.previous,
                units_line(&installed.units)
            );
        }
    }
    Ok(())
}

fn agent_exec(args: AgentExecArgs) -> ExitCode {
    let policy = match Policy::load(&args.policy) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            return ExitCode::from(EXIT_CANNOT_EXECUTE);
        }
    };
    let target = Target {
        uid: args.uid,
        program: args.program,
        args: args.args,
    };
    let error = exec::run(&policy, &target);
    eprintln!("error: {}", describe(&error));
    ExitCode::from(EXIT_CANNOT_EXECUTE)
}

fn units_line(units: &[String]) -> String {
    if units.is_empty() {
        String::new()
    } else {
        format!("; units refreshed: {}", units.join(", "))
    }
}

fn load(path: Option<&Path>) -> Result<Config, Error> {
    let path = path.map_or_else(Config::default_path, Into::into);
    Ok(Config::load(&path)?)
}

fn applied(names: &[String]) -> String {
    if names.is_empty() {
        "up to date".to_owned()
    } else {
        format!("applied {}", names.join(", "))
    }
}

async fn signals() {
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(terminate) => Some(terminate),
        Err(error) => {
            tracing::warn!(error = %error, "SIGTERM cannot be observed; only SIGINT stops the service");
            None
        }
    };
    let term = async {
        match terminate.as_mut() {
            Some(terminate) => {
                terminate.recv().await;
            }
            None => std::future::pending::<()>().await,
        }
    };
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %error, "SIGINT cannot be observed");
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        _ = term => tracing::info!("SIGTERM received"),
        _ = interrupt => tracing::info!("SIGINT received"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_subcommands_parse() {
        let cli = Cli::try_parse_from(["keel", "serve"]).unwrap();
        assert!(matches!(cli.command, Command::Serve));
        assert!(cli.config.is_none());
        let cli = Cli::try_parse_from(["keel", "--config", "/tmp/k.yaml", "migrate"]).unwrap();
        assert!(matches!(cli.command, Command::Migrate));
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/k.yaml")));
        let cli = Cli::try_parse_from(["keel", "product", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Product {
                action: ProductAction::List
            }
        ));
        let cli = Cli::try_parse_from(["keel", "product", "features"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Product {
                action: ProductAction::Features
            }
        ));
        let cli = Cli::try_parse_from(["keel", "product", "enable", "demo"]).unwrap();
        match cli.command {
            Command::Product {
                action: ProductAction::Enable { id },
            } => assert_eq!(id, "demo"),
            other => panic!("{other:?}"),
        }
        let cli = Cli::try_parse_from(["keel", "product", "disable", "demo", "--config", "c.yaml"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Product {
                action: ProductAction::Disable {
                    switch_remote: false,
                    ..
                }
            }
        ));
        assert_eq!(cli.config, Some(PathBuf::from("c.yaml")));
        let cli =
            Cli::try_parse_from(["keel", "product", "disable", "dns", "--switch-remote"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Product {
                action: ProductAction::Disable {
                    switch_remote: true,
                    ..
                }
            }
        ));
    }

    #[test]
    fn license_install_takes_the_file_and_the_ctl_options() {
        let cli =
            Cli::try_parse_from(["keel", "license", "install", "/etc/keel/license.lease"]).unwrap();
        let Command::License(args) = cli.command else {
            panic!("{:?}", cli.command);
        };
        let LicenseAction::Install { file } = &args.action;
        assert_eq!(file, &PathBuf::from("/etc/keel/license.lease"));
        assert_eq!(args.out, OutArg::Json);
        assert_eq!(args.socket, None);
        assert_eq!(args.lang, "en");
        let cli = Cli::try_parse_from([
            "keel",
            "license",
            "install",
            "lease.json",
            "--socket",
            "/run/keel/core.sock",
            "--out",
            "table",
            "--lang",
            "ru",
        ])
        .unwrap();
        let Command::License(args) = cli.command else {
            panic!("{:?}", cli.command);
        };
        assert_eq!(args.socket, Some(PathBuf::from("/run/keel/core.sock")));
        assert_eq!(args.out, OutArg::Table);
        assert_eq!(args.lang, "ru");
        assert!(Cli::try_parse_from(["keel", "license", "install"]).is_err());
    }

    #[test]
    fn ctl_takes_the_function_the_pairs_and_its_options() {
        let cli = Cli::try_parse_from([
            "keel",
            "ctl",
            "note.edit",
            "title=x",
            "ttl=300",
            "--out",
            "table",
            "--socket",
            "/tmp/core.sock",
            "--lang",
            "ru",
            "--as",
            "alice",
        ])
        .unwrap();
        let Command::Ctl(args) = cli.command else {
            panic!("expected ctl");
        };
        assert_eq!(args.func, "note.edit");
        assert_eq!(args.pairs, ["title=x", "ttl=300"]);
        assert_eq!(args.out, OutArg::Table);
        assert_eq!(Out::from(args.out), Out::Table);
        assert_eq!(args.socket, Some(PathBuf::from("/tmp/core.sock")));
        assert_eq!(args.lang, "ru");
        assert_eq!(args.run_as.as_deref(), Some("alice"));
        let cli = Cli::try_parse_from(["keel", "ctl", "keel.account.list"]).unwrap();
        let Command::Ctl(args) = cli.command else {
            panic!("expected ctl");
        };
        assert!(args.pairs.is_empty());
        assert_eq!(args.out, OutArg::Json);
        assert!(args.socket.is_none());
        assert_eq!(args.lang, "en");
        assert!(args.run_as.is_none());
        assert!(Cli::try_parse_from(["keel", "ctl"]).is_err());
        assert!(Cli::try_parse_from(["keel", "ctl", "a.b", "--out", "yaml"]).is_err());
        assert!(Cli::try_parse_from(["keel"]).is_err());
    }

    #[test]
    fn update_takes_a_step_and_the_ctl_options_after_it() {
        let cli = Cli::try_parse_from(["keel", "update", "check", "--out", "json"]).unwrap();
        let Command::Update(args) = cli.command else {
            panic!("expected update");
        };
        assert!(matches!(args.step, UpdateStep::Check));
        assert_eq!(args.step.function(), UPDATE_CHECK);
        assert!(!args.step.waits());
        assert_eq!(args.out, OutArg::Json);
        assert!(args.socket.is_none());
        assert_eq!(args.lang, "en");
        let cli = Cli::try_parse_from([
            "keel",
            "update",
            "apply",
            "--wait",
            "--out",
            "table",
            "--socket",
            "/tmp/core.sock",
            "--lang",
            "ru",
        ])
        .unwrap();
        let Command::Update(args) = cli.command else {
            panic!("expected update");
        };
        assert!(matches!(args.step, UpdateStep::Apply { wait: true }));
        assert_eq!(args.step.function(), UPDATE_APPLY);
        assert!(args.step.waits());
        assert_eq!(args.out, OutArg::Table);
        assert_eq!(args.socket, Some(PathBuf::from("/tmp/core.sock")));
        assert_eq!(args.lang, "ru");
        let cli = Cli::try_parse_from(["keel", "update", "rollback"]).unwrap();
        let Command::Update(args) = cli.command else {
            panic!("expected update");
        };
        assert!(matches!(args.step, UpdateStep::Rollback { wait: false }));
        assert_eq!(args.step.function(), UPDATE_ROLLBACK);
        assert!(!args.step.waits());
        assert!(Cli::try_parse_from(["keel", "update"]).is_err());
        assert!(Cli::try_parse_from(["keel", "update", "check", "--wait"]).is_err());
        assert!(Cli::try_parse_from(["keel", "update", "install"]).is_err());
    }

    #[test]
    fn privileged_takes_the_socket_and_policy_and_update_takes_the_request_or_rollback() {
        let cli = Cli::try_parse_from(["keel", "privileged"]).unwrap();
        let Command::Privileged(args) = cli.command else {
            panic!("expected privileged");
        };
        assert_eq!(args.socket, PathBuf::from("/run/keel/agent.sock"));
        assert_eq!(args.policy, PathBuf::from("/etc/keel/agent.yaml"));
        assert_eq!(args.user, "keel");
        assert_eq!(args.templates, PathBuf::from("/etc/keel/templates"));
        assert!(args.action.is_none());
        let cli = Cli::try_parse_from([
            "keel",
            "privileged",
            "--socket",
            "/tmp/agent.sock",
            "--policy",
            "/tmp/agent.yaml",
            "--user",
            "tester",
            "--templates",
            "/tmp/templates",
        ])
        .unwrap();
        let Command::Privileged(args) = cli.command else {
            panic!("expected privileged");
        };
        assert_eq!(args.socket, PathBuf::from("/tmp/agent.sock"));
        assert_eq!(args.policy, PathBuf::from("/tmp/agent.yaml"));
        assert_eq!(args.user, "tester");
        assert_eq!(args.templates, PathBuf::from("/tmp/templates"));

        let cli = Cli::try_parse_from(["keel", "privileged", "update", "--from-request"]).unwrap();
        let Command::Privileged(PrivilegedArgs {
            action: Some(PrivilegedAction::Update(update)),
            ..
        }) = cli.command
        else {
            panic!("expected update");
        };
        assert!(update.from_request);
        assert!(update.action.is_none());
        assert_eq!(
            update.request,
            PathBuf::from("/var/lib/keel/update.request")
        );
        assert_eq!(update.channel_url, "https://dl.globalart.hu/keel/");
        assert_eq!(update.opt, PathBuf::from("/opt/keel"));
        assert_eq!(update.unit_dir, PathBuf::from("/etc/systemd/system"));
        assert_eq!(update.systemctl, PathBuf::from("systemctl"));
        assert!(update.release_key.is_none());

        let cli = Cli::try_parse_from([
            "keel",
            "privileged",
            "update",
            "--request",
            "/tmp/r",
            "--channel-url",
            "/tmp/channel",
            "--opt",
            "/tmp/opt",
            "--unit-dir",
            "/tmp/units",
            "--systemctl",
            "/tmp/systemctl",
            "--release-key",
            "/tmp/k.pub",
            "rollback",
            "--restore",
            "/tmp/backup.tar.zst",
        ])
        .unwrap();
        let Command::Privileged(PrivilegedArgs {
            action: Some(PrivilegedAction::Update(update)),
            ..
        }) = cli.command
        else {
            panic!("expected update");
        };
        assert!(!update.from_request);
        assert_eq!(update.request, PathBuf::from("/tmp/r"));
        assert_eq!(update.channel_url, "/tmp/channel");
        assert_eq!(update.opt, PathBuf::from("/tmp/opt"));
        assert_eq!(update.unit_dir, PathBuf::from("/tmp/units"));
        assert_eq!(update.systemctl, PathBuf::from("/tmp/systemctl"));
        assert_eq!(update.release_key, Some(PathBuf::from("/tmp/k.pub")));
        assert!(matches!(
            update.action,
            Some(UpdateAction::Rollback { restore: Some(archive) }) if archive == Path::new("/tmp/backup.tar.zst")
        ));

        let cli = Cli::try_parse_from(["keel", "privileged", "update", "rollback"]).unwrap();
        let Command::Privileged(PrivilegedArgs {
            action: Some(PrivilegedAction::Update(update)),
            ..
        }) = cli.command
        else {
            panic!("expected update");
        };
        assert!(matches!(
            update.action,
            Some(UpdateAction::Rollback { restore: None })
        ));

        assert!(Cli::try_parse_from(["keel", "privileged", "update"]).is_err());
        assert!(Cli::try_parse_from(["keel", "privileged", "host"]).is_err());
        assert!(
            Cli::try_parse_from(["keel", "privileged", "update", "--from-request", "extra"])
                .is_err()
        );
    }

    #[test]
    fn agent_exec_takes_the_uid_the_policy_and_the_program_with_its_arguments() {
        let cli = Cli::try_parse_from([
            "keel",
            "agent-exec",
            "--uid",
            "100001",
            "--policy",
            "/tmp/agent.yaml",
            "--",
            "/usr/bin/id",
            "-u",
            "--zero",
        ])
        .unwrap();
        let Command::AgentExec(args) = cli.command else {
            panic!("expected agent-exec");
        };
        assert_eq!(args.uid, 100_001);
        assert_eq!(args.policy, PathBuf::from("/tmp/agent.yaml"));
        assert_eq!(args.program, PathBuf::from("/usr/bin/id"));
        assert_eq!(args.args, ["-u", "--zero"]);
        let cli = Cli::try_parse_from(["keel", "agent-exec", "--uid", "7", "/bin/true"]).unwrap();
        let Command::AgentExec(args) = cli.command else {
            panic!("expected agent-exec");
        };
        assert_eq!(args.policy, PathBuf::from("/etc/keel/agent.yaml"));
        assert!(args.args.is_empty());
        assert!(Cli::try_parse_from(["keel", "agent-exec", "/bin/true"]).is_err());
        assert!(Cli::try_parse_from(["keel", "agent-exec", "--uid", "7"]).is_err());
        assert!(Cli::try_parse_from(["keel", "agent-exec", "--uid", "-1", "/bin/true"]).is_err());
    }

    #[test]
    fn agent_home_takes_every_argument_after_it_as_the_operation_of_the_worker() {
        let cli = Cli::try_parse_from([
            "keel",
            "agent-home",
            "--home",
            "/var/www/keel-panel-a",
            "rename",
            "/var/www/keel-panel-a/-x",
            "/var/www/keel-panel-a/y",
        ])
        .unwrap();
        let Command::AgentHome(args) = cli.command else {
            panic!("expected agent-home");
        };
        assert_eq!(
            args.args,
            [
                "--home",
                "/var/www/keel-panel-a",
                "rename",
                "/var/www/keel-panel-a/-x",
                "/var/www/keel-panel-a/y"
            ]
        );
    }

    #[test]
    fn backup_takes_its_output_and_key_flags_and_restore_takes_the_archive() {
        let cli = Cli::try_parse_from(["keel", "backup"]).unwrap();
        let Command::Backup(args) = cli.command else {
            panic!("expected backup");
        };
        assert!(args.out.is_none());
        assert!(!args.with_secret_key);
        assert!(args.action.is_none());
        let cli = Cli::try_parse_from([
            "keel",
            "backup",
            "--out",
            "/tmp/site.tar.zst",
            "--with-secret-key",
            "--passphrase",
            "words",
        ])
        .unwrap();
        let Command::Backup(args) = cli.command else {
            panic!("expected backup");
        };
        assert_eq!(args.out, Some(PathBuf::from("/tmp/site.tar.zst")));
        assert!(args.with_secret_key);
        assert_eq!(args.passphrase.as_deref(), Some("words"));
        let cli = Cli::try_parse_from(["keel", "backup", "inspect", "/tmp/site.tar.zst"]).unwrap();
        let Command::Backup(args) = cli.command else {
            panic!("expected backup");
        };
        assert!(matches!(
            args.action,
            Some(BackupAction::Inspect { archive }) if archive == Path::new("/tmp/site.tar.zst")
        ));
        let cli =
            Cli::try_parse_from(["keel", "restore", "/tmp/site.tar.zst", "--with-secret-key"])
                .unwrap();
        let Command::Restore(args) = cli.command else {
            panic!("expected restore");
        };
        assert_eq!(args.archive, PathBuf::from("/tmp/site.tar.zst"));
        assert!(args.with_secret_key);
        assert!(args.passphrase.is_none());
        assert!(Cli::try_parse_from(["keel", "restore"]).is_err());
        assert!(Cli::try_parse_from(["keel", "backup", "inspect"]).is_err());
        assert!(Cli::try_parse_from(["keel", "backup", "--out"]).is_err());
        assert_eq!(list_or_none(&[]), "none");
        assert_eq!(list_or_none(&["a".into(), "b".into()]), "a, b");
    }

    #[test]
    fn the_doctor_reaches_the_agent_at_the_socket_of_the_host_section_unless_the_flag_names_one() {
        let config =
            Config::parse(Some("host: { socket: /srv/site/agent.sock }\n"), |_| None).unwrap();
        let bare = Config::parse(None, |_| None).unwrap();
        let args = |argv: &[&str]| {
            let Command::Doctor(args) = Cli::try_parse_from(argv).unwrap().command else {
                panic!("expected doctor");
            };
            args.layout
        };
        assert_eq!(
            args(&["keel", "doctor"]).of_site(&config).agent_socket,
            PathBuf::from("/srv/site/agent.sock")
        );
        assert_eq!(
            args(&["keel", "doctor"]).of_site(&bare).agent_socket,
            PathBuf::from("/run/keel/agent.sock")
        );
        assert_eq!(
            args(&["keel", "doctor", "--agent-socket", "/tmp/flag.sock"])
                .of_site(&config)
                .agent_socket,
            PathBuf::from("/tmp/flag.sock")
        );
    }

    #[test]
    fn doctor_and_support_bundle_take_the_layout_flags_with_the_design_defaults() {
        let cli = Cli::try_parse_from(["keel", "doctor"]).unwrap();
        let Command::Doctor(args) = cli.command else {
            panic!("expected doctor");
        };
        assert_eq!(args.out, ReportOut::Text);
        let layout = Layout::from(args.layout);
        assert_eq!(layout, Layout::default());
        assert_eq!(layout.opt, PathBuf::from("/opt/keel"));
        assert_eq!(layout.unit_dir, PathBuf::from("/etc/systemd/system"));
        assert_eq!(layout.systemctl, PathBuf::from("systemctl"));
        assert_eq!(layout.agent_socket, PathBuf::from("/run/keel/agent.sock"));
        let cli = Cli::try_parse_from([
            "keel",
            "doctor",
            "--out",
            "json",
            "--opt",
            "/tmp/opt",
            "--unit-dir",
            "/tmp/units",
            "--systemctl",
            "/tmp/bin/systemctl",
            "--agent-socket",
            "/tmp/agent.sock",
        ])
        .unwrap();
        let Command::Doctor(args) = cli.command else {
            panic!("expected doctor");
        };
        assert_eq!(args.out, ReportOut::Json);
        let layout = Layout::from(args.layout);
        assert_eq!(layout.opt, PathBuf::from("/tmp/opt"));
        assert_eq!(layout.unit_dir, PathBuf::from("/tmp/units"));
        assert_eq!(layout.systemctl, PathBuf::from("/tmp/bin/systemctl"));
        assert_eq!(layout.agent_socket, PathBuf::from("/tmp/agent.sock"));
        let cli = Cli::try_parse_from([
            "keel",
            "support-bundle",
            "--out",
            "/tmp/bundle.tar.zst",
            "--unit-dir",
            "/tmp/units",
        ])
        .unwrap();
        let Command::SupportBundle(args) = cli.command else {
            panic!("expected support-bundle");
        };
        assert_eq!(args.out, Some(PathBuf::from("/tmp/bundle.tar.zst")));
        assert_eq!(args.layout.unit_dir, PathBuf::from("/tmp/units"));
        assert_eq!(args.layout.opt, PathBuf::from("/opt/keel"));
        let cli = Cli::try_parse_from(["keel", "support-bundle"]).unwrap();
        let Command::SupportBundle(args) = cli.command else {
            panic!("expected support-bundle");
        };
        assert!(args.out.is_none());
        assert!(Cli::try_parse_from(["keel", "doctor", "--out", "yaml"]).is_err());
        assert!(Cli::try_parse_from(["keel", "doctor", "extra"]).is_err());
        assert!(Cli::try_parse_from(["keel", "support-bundle", "--out"]).is_err());
    }

    #[test]
    fn the_kernel_unit_restarts_the_service_on_the_restart_exit_status() {
        let unit = include_str!("../../../deploy/systemd/keel.service");
        let status = restart::EXIT_STATUS;
        assert!(unit.lines().any(|line| line == "Restart=always"), "{unit}");
        assert!(
            unit.lines()
                .any(|line| line == format!("RestartForceExitStatus={status}")),
            "{unit}"
        );
        assert!(
            unit.lines()
                .any(|line| line == format!("SuccessExitStatus={status}")),
            "{unit}"
        );
        assert_ne!(status, 0);
        assert_ne!(status, 1);
        assert_ne!(status, 2);
    }

    #[test]
    fn a_usage_error_exits_with_the_clap_code_and_no_panic() {
        let code = Runtime::new().run_from(["keel", "nothing"]);
        assert_eq!(code, ExitCode::from(2));
        let help = Runtime::new().run_from(["keel", "--version"]);
        assert_eq!(help, ExitCode::SUCCESS);
    }
}
