use clap::Parser;

use agent_sim::cli::args::CliArgs;
use agent_sim::daemon::lifecycle::bootstrap_daemon;
use agent_sim::envd::spec::read_env_spec;
use agent_sim::load::read_load_spec;
use agent_sim::{cli, daemon, envd};

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
    if args.bootstrap_instance {
        let Some(load_spec_path) = args.load_spec_path.as_deref() else {
            eprintln!("bootstrap-instance mode requires --load-spec-path");
            return std::process::ExitCode::from(1);
        };
        let load_spec = match read_load_spec(std::path::Path::new(load_spec_path)) {
            Ok(load_spec) => load_spec,
            Err(err) => {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
        };
        let _ = std::fs::remove_file(load_spec_path);
        if let Err(err) = bootstrap_daemon(&args.instance, &load_spec).await {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }
    if args.env_daemon {
        let Some(env_spec_path) = args.env_spec_path.as_deref() else {
            eprintln!("env daemon mode requires --env-spec-path");
            return std::process::ExitCode::from(1);
        };
        let env_spec = match read_env_spec(std::path::Path::new(env_spec_path)) {
            Ok(env_spec) => env_spec,
            Err(err) => {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
        };
        let _ = std::fs::remove_file(env_spec_path);
        if let Err(err) = envd::run(env_spec).await {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }
    if args.daemon {
        let Some(load_spec_path) = args.load_spec_path.as_deref() else {
            eprintln!("daemon mode requires --load-spec-path");
            return std::process::ExitCode::from(1);
        };
        let load_spec = match read_load_spec(std::path::Path::new(load_spec_path)) {
            Ok(load_spec) => load_spec,
            Err(err) => {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
        };
        let _ = std::fs::remove_file(load_spec_path);
        if let Err(err) = daemon::run(&args.instance, load_spec).await {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }

    cli::run_with_args(args).await
}
