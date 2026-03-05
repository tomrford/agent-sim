use clap::Parser;

use agent_sim::cli::args::CliArgs;
use agent_sim::sim::init::InitConfig;
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
        let init_config = match args.init_config_json.as_deref() {
            Some(raw) => match serde_json::from_str::<InitConfig>(raw) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!("invalid --init-config-json payload: {err}");
                    return std::process::ExitCode::from(1);
                }
            },
            None => InitConfig::default(),
        };
        if let Err(err) =
            daemon::run(&args.session, libpath, args.env_tag.clone(), init_config).await
        {
            eprintln!("{err}");
            return std::process::ExitCode::from(1);
        }
        return std::process::ExitCode::SUCCESS;
    }

    cli::run_with_args(args).await
}
