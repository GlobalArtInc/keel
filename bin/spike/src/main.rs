mod agent;
mod client;
mod ops;
mod policy;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

pub const EXIT_REFUSED: u8 = 1;
pub const EXIT_FAILED: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "keel-spike",
    version,
    about = "The host spike: a root agent and its client"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Agent(agent::Args),
    Call(client::Args),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Agent(args) => match agent::run(args).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("keel-spike agent: {error}");
                ExitCode::from(EXIT_FAILED)
            }
        },
        Command::Call(args) => match client::run(args).await {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(EXIT_REFUSED),
            Err(error) => {
                eprintln!("keel-spike call: {error}");
                ExitCode::from(EXIT_FAILED)
            }
        },
    }
}
