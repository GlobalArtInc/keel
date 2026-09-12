use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use keel_kernel::ctl::{self, Out};
use keel_kernel::{Config, logging};
use tokio::signal::unix::{SignalKind, signal};

use crate::VERSION;
use crate::error::{Error, describe};
use crate::runtime::Runtime;

#[derive(Debug, Parser)]
#[command(name = "keel", version = VERSION, about = "The keel installation: serve, migrate, product and ctl")]
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
                    };
                    let output = ctl::run(&socket, &request).await?;
                    if output.ok {
                        println!("{}", output.text.trim_end_matches('\n'));
                        Ok(ExitCode::SUCCESS)
                    } else {
                        eprintln!("{}", output.text.trim_end_matches('\n'));
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
        })
    }
}

fn load(path: Option<&std::path::Path>) -> Result<Config, Error> {
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
        let cli = Cli::try_parse_from(["keel", "ctl", "keel.account.list"]).unwrap();
        let Command::Ctl(args) = cli.command else {
            panic!("expected ctl");
        };
        assert!(args.pairs.is_empty());
        assert_eq!(args.out, OutArg::Json);
        assert!(args.socket.is_none());
        assert_eq!(args.lang, "en");
        assert!(Cli::try_parse_from(["keel", "ctl"]).is_err());
        assert!(Cli::try_parse_from(["keel", "ctl", "a.b", "--out", "yaml"]).is_err());
        assert!(Cli::try_parse_from(["keel"]).is_err());
        assert!(Cli::try_parse_from(["keel", "privileged"]).is_err());
    }

    #[test]
    fn a_usage_error_exits_with_the_clap_code_and_no_panic() {
        let code = Runtime::new().run_from(["keel", "nothing"]);
        assert_eq!(code, ExitCode::from(2));
        let help = Runtime::new().run_from(["keel", "--version"]);
        assert_eq!(help, ExitCode::SUCCESS);
    }
}
