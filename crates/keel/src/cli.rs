use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};
use keel_kernel::ctl::{self, Out};
use keel_kernel::{Config, logging};
use keel_release::{KeyRing, PublicKey};
use keel_types::TaskState;
use serde_json::{Value, json};
use tokio::signal::unix::{SignalKind, signal};

use crate::VERSION;
use crate::backup::{self, BackupCliError, RESTORE_CLI, describe_manifest};
use crate::error::{Error, describe};
use crate::privileged::{self, PrivilegedError, Request, Runner, Source};
use crate::runtime::Runtime;

const UPDATE_CHECK: &str = "update.check";
const UPDATE_APPLY: &str = "update.apply";
const UPDATE_ROLLBACK: &str = "update.rollback";
const TASK_GET: &str = "task.get";
const TASK_POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Parser)]
#[command(name = "keel", version = VERSION, about = "The keel installation: serve, migrate, product, ctl, update, backup, restore and privileged")]
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
    #[command(about = "List, enable or disable product bundles while the service is stopped")]
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
    #[command(about = "Write a backup archive of the installation, or inspect the manifest of one")]
    Backup(BackupArgs),
    #[command(about = "Restore a backup archive while the service is stopped")]
    Restore(RestoreArgs),
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
    #[command(about = "Enable a bundle; its modules load at the next start")]
    Enable { id: String },
    #[command(about = "Disable a bundle; its modules become orphaned at the next start")]
    Disable { id: String },
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
        let executor = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(Error::Executor)?;
        executor.block_on(async {
            match cli.command {
                Command::Serve => {
                    let config = load(cli.config.as_deref())?;
                    logging::init(&config.log)?;
                    self.serve(&config, signals()).await?;
                    Ok(ExitCode::SUCCESS)
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
                    match action {
                        ProductAction::List => {
                            for state in self.list_products(&config).await? {
                                println!("{state}");
                            }
                        }
                        ProductAction::Enable { id } => {
                            let change = self.enable_product(&config, &id).await?;
                            println!("{id}: {change}");
                        }
                        ProductAction::Disable { id } => {
                            let change = self.disable_product(&config, &id).await?;
                            println!("{id}: {change}");
                        }
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Command::Privileged(args) => match args.action {
                    None => {
                        privileged::serve(&args.socket, &args.policy, signals()).await;
                        Ok(ExitCode::SUCCESS)
                    }
                    Some(PrivilegedAction::Update(update)) => Ok(privileged_update(update).await),
                },
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
                Command::Backup(args) => {
                    let config = load(cli.config.as_deref())?;
                    Ok(report(self.backup_command(&config, args).await))
                }
                Command::Restore(args) => {
                    let config = load(cli.config.as_deref())?;
                    Ok(report(self.restore_command(&config, args).await))
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
                action: ProductAction::Disable { .. }
            }
        ));
        assert_eq!(cli.config, Some(PathBuf::from("c.yaml")));
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
        assert!(args.action.is_none());
        let cli = Cli::try_parse_from([
            "keel",
            "privileged",
            "--socket",
            "/tmp/agent.sock",
            "--policy",
            "/tmp/agent.yaml",
        ])
        .unwrap();
        let Command::Privileged(args) = cli.command else {
            panic!("expected privileged");
        };
        assert_eq!(args.socket, PathBuf::from("/tmp/agent.sock"));
        assert_eq!(args.policy, PathBuf::from("/tmp/agent.yaml"));

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
    fn a_usage_error_exits_with_the_clap_code_and_no_panic() {
        let code = Runtime::new().run_from(["keel", "nothing"]);
        assert_eq!(code, ExitCode::from(2));
        let help = Runtime::new().run_from(["keel", "--version"]);
        assert_eq!(help, ExitCode::SUCCESS);
    }
}
