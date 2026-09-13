mod client;
mod report;
mod suite;

use std::process::ExitCode;

use clap::Parser;

use suite::Settings;

#[derive(Parser)]
#[command(
    name = "keel-conformance",
    version,
    about = "Runs the contract suite against a started keel binary and prints one line per check"
)]
struct Args {
    #[arg(long, env = "KEEL_CONFORMANCE_URL")]
    url: String,
    #[arg(long, env = "KEEL_CONFORMANCE_ADMIN_LOGIN")]
    admin_login: String,
    #[arg(long, env = "KEEL_CONFORMANCE_ADMIN_PASSWORD", hide_env_values = true)]
    admin_password: String,
    #[arg(long, env = "KEEL_CONFORMANCE_USER_LOGIN")]
    user_login: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let settings = Settings {
        url: args.url,
        admin_login: args.admin_login,
        admin_password: args.admin_password,
        user_login: args.user_login,
    };
    match suite::run(settings).await {
        Ok(outcome) => {
            println!("{outcome}");
            if outcome.is_green() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("keel-conformance: {error}");
            ExitCode::from(2)
        }
    }
}
