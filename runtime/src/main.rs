use clap::Parser;

use agent_sim::cli::args::CliArgs;
use agent_sim::daemon::lifecycle::bootstrap_daemon;
use agent_sim::envd::spec::read_env_spec;
use agent_sim::internal_cli::{InternalCommand, parse_from_env_if_internal};
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

    if let Some(command) = parse_from_env_if_internal() {
        return run_internal(command).await;
    }

    let args = CliArgs::parse();
    cli::run_with_args(args).await
}

async fn run_internal(command: InternalCommand) -> std::process::ExitCode {
    match command {
        InternalCommand::BootstrapInstance {
            instance,
            load_spec_path,
        } => {
            let load_spec = match read_load_spec(std::path::Path::new(&load_spec_path)) {
                Ok(load_spec) => load_spec,
                Err(err) => {
                    eprintln!("{err}");
                    return std::process::ExitCode::from(1);
                }
            };
            let _ = std::fs::remove_file(&load_spec_path);
            if let Err(err) = bootstrap_daemon(&instance, &load_spec).await {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
            std::process::ExitCode::SUCCESS
        }
        InternalCommand::EnvDaemon { env_spec_path } => {
            let env_spec = match read_env_spec(std::path::Path::new(&env_spec_path)) {
                Ok(env_spec) => env_spec,
                Err(err) => {
                    eprintln!("{err}");
                    return std::process::ExitCode::from(1);
                }
            };
            let _ = std::fs::remove_file(&env_spec_path);
            if let Err(err) = envd::run(env_spec).await {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
            std::process::ExitCode::SUCCESS
        }
        InternalCommand::InstanceDaemon {
            instance,
            load_spec_path,
        } => {
            let load_spec = match read_load_spec(std::path::Path::new(&load_spec_path)) {
                Ok(load_spec) => load_spec,
                Err(err) => {
                    eprintln!("{err}");
                    return std::process::ExitCode::from(1);
                }
            };
            let _ = std::fs::remove_file(&load_spec_path);
            if let Err(err) = daemon::run(&instance, load_spec).await {
                eprintln!("{err}");
                return std::process::ExitCode::from(1);
            }
            std::process::ExitCode::SUCCESS
        }
    }
}
