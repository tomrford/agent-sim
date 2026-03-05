use clap::Parser;

use agent_sim::cli::args::CliArgs;
use agent_sim::{cli, daemon};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent_sim=info".into()),
        )
        .with_target(false)
        .try_init();

    let args = CliArgs::parse();
    if args.daemon {
        let Some(libpath) = args.libpath.as_deref() else {
            eprintln!("daemon mode requires --libpath");
            return std::process::ExitCode::from(1);
        };
        if let Err(err) = daemon::run(&args.session, libpath, args.env_tag.clone()).await {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }

    cli::run_with_args(args).await
}
