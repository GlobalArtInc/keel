mod types;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "keel-dev",
    version,
    about = "The developer CLI of the platform"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(
        about = "Writes the JSON Schema of the wire contract: one document and one $ref file per root type"
    )]
    Types {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        check: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Types { out, check } => types::run(&out, check),
    };
    match outcome {
        Ok(outcome) => {
            println!("{outcome}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("keel-dev: {error}");
            ExitCode::FAILURE
        }
    }
}
