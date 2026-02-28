use clap::Parser;

use agent_sim::cli::args::CliArgs;
use agent_sim::{cli, daemon};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = CliArgs::parse();
    if args.daemon {
        if let Err(err) = daemon::run(&args.session).await {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }

    cli::run_with_args(args).await
}
