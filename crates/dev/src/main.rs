mod descriptors;
mod openapi;
mod types;
mod views;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use descriptors::Login;

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
    #[command(
        subcommand,
        about = "The per-level dump of a started binary: boot, descriptors, catalogues and product rows"
    )]
    Descriptors(Descriptors),
    #[command(
        subcommand,
        about = "The share of custom views among the functions of each product and of the composition"
    )]
    Views(Views),
    #[command(
        about = "Writes the OpenAPI description of the admin dump: one POST /api/v1/call/<func> per function"
    )]
    Openapi {
        #[arg(long)]
        dump: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum Descriptors {
    #[command(
        about = "Writes <out>/<level>/ for the guest and for every --as level, logging in through auth.login"
    )]
    Dump {
        #[arg(long)]
        url: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long = "as", value_name = "LEVEL=LOGIN:PASSWORD")]
        logins: Vec<Login>,
    },
    #[command(
        about = "Prints a per-file unified diff of two dump directories and exits non-zero on any difference"
    )]
    Diff { a: PathBuf, b: PathBuf },
    #[command(about = "Dumps to a temporary directory and diffs the committed <dir> against it")]
    Check {
        #[arg(long)]
        url: String,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long = "as", value_name = "LEVEL=LOGIN:PASSWORD")]
        logins: Vec<Login>,
    },
}

#[derive(Subcommand)]
enum Views {
    #[command(
        about = "Prints one line per product and one for the composition; a share above 20 % exits non-zero"
    )]
    Share {
        #[arg(long)]
        dump: PathBuf,
        #[arg(long)]
        frontend: PathBuf,
    },
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Types(#[from] types::Error),
    #[error(transparent)]
    Descriptors(#[from] descriptors::Error),
    #[error(transparent)]
    Views(#[from] views::Error),
    #[error(transparent)]
    Openapi(#[from] openapi::Error),
}

async fn run(command: Command) -> Result<bool, Error> {
    match command {
        Command::Types { out, check } => {
            println!("{}", types::run(&out, check)?);
            Ok(true)
        }
        Command::Descriptors(Descriptors::Dump { url, out, logins }) => {
            println!("{}", descriptors::dump(&url, &out, &logins).await?);
            Ok(true)
        }
        Command::Descriptors(Descriptors::Diff { a, b }) => report(descriptors::diff(&a, &b)?),
        Command::Descriptors(Descriptors::Check { url, dir, logins }) => {
            report(descriptors::check(&url, &dir, &logins).await?)
        }
        Command::Views(Views::Share { dump, frontend }) => {
            let shares = views::share(&dump, &frontend)?;
            print!("{shares}");
            Ok(shares.green())
        }
        Command::Openapi { dump, out, check } => {
            let outcome = openapi::run(&dump, &out, check)?;
            println!("{outcome}");
            Ok(outcome.current())
        }
    }
}

fn report(diff: descriptors::Diff) -> Result<bool, Error> {
    print!("{diff}");
    println!("{}", diff.summary());
    Ok(diff.is_empty())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("keel-dev: {error}");
            ExitCode::FAILURE
        }
    }
}
