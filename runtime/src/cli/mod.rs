pub mod args;
pub mod commands;
mod env;
pub mod error;
pub mod output;
mod recipe;

use crate::cli::args::{CliArgs, CloseArgs, Command, WatchArgs};
use crate::cli::commands::to_request;
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::connection::{send_env_request, send_request};
use crate::daemon::lifecycle::{self, bootstrap_daemon};
use crate::envd::lifecycle as env_lifecycle;
use crate::load::resolve::resolve_standalone_load_spec;
use crate::protocol::{Action, Request, Response, ResponseData, SignalValueData, WatchSampleData};
use std::path::Path;
use std::process::ExitCode;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

pub async fn run_with_args(args: CliArgs) -> ExitCode {
    match run_inner(args).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

async fn run_inner(args: CliArgs) -> Result<ExitCode, CliError> {
    let Some(command) = args.command.as_ref() else {
        return Err(CliError::MissingCommand);
    };

    match command {
        Command::Load(load) => run_load_command(&args, load).await,
        Command::Watch(watch) => run_watch_command(&args, watch).await,
        Command::Run(run) => recipe::run_recipe_command(&args, run).await,
        Command::Close(close) if close.all || close.env.is_some() => run_close_command(close).await,
        Command::Env(env) => env::run_env_command(&args, env).await,
        _ => {
            let request = to_request(&args)?;
            let response = send_request(&args.instance, &request)
                .await
                .map_err(|e| CliError::CommandFailed(e.to_string()))?;
            output::print_response(&response, args.json);
            if response.success {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
    }
}

async fn run_load_command(
    args: &CliArgs,
    load: &crate::cli::args::LoadArgs,
) -> Result<ExitCode, CliError> {
    let config = load_config(args.config.as_deref().map(Path::new))
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    let config_base_dir = config.source_path.as_ref().and_then(|path| path.parent());
    let load_spec = resolve_standalone_load_spec(
        &config.file,
        config_base_dir,
        load.libpath.as_deref(),
        &load.flash,
        args.env_tag.clone(),
    )
    .map_err(|err| CliError::CommandFailed(err.to_string()))?;

    bootstrap_daemon(&args.instance, &load_spec)
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    let response = send_action(&args.instance, Action::Info).await?;
    let ResponseData::ProjectInfo {
        libpath,
        signal_count,
        ..
    } = response
        .data
        .ok_or_else(|| CliError::CommandFailed("missing info response payload".to_string()))?
    else {
        return Err(CliError::CommandFailed(
            "unexpected daemon response after load".to_string(),
        ));
    };
    let response = Response::ok(
        Uuid::new_v4(),
        ResponseData::Loaded {
            libpath,
            signal_count,
        },
    );
    output::print_response(&response, args.json);
    Ok(ExitCode::SUCCESS)
}

async fn run_watch_command(args: &CliArgs, watch: &WatchArgs) -> Result<ExitCode, CliError> {
    let count = watch.samples.unwrap_or(10).max(1);
    let mut samples = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let (tick, time_us, signal_value) =
            fetch_signal_sample(&args.instance, &watch.selector).await?;
        samples.push(WatchSampleData {
            tick,
            time_us,
            signal: signal_value.name,
            value: signal_value.value,
        });
        if idx + 1 < count {
            sleep(Duration::from_millis(watch.interval_ms.max(1))).await;
        }
    }

    let response = Response::ok(Uuid::new_v4(), ResponseData::WatchSamples { samples });
    output::print_response(&response, args.json);
    Ok(ExitCode::SUCCESS)
}

async fn run_close_command(close: &CloseArgs) -> Result<ExitCode, CliError> {
    if let Some(env_name) = &close.env {
        close_env_and_wait(env_name).await?;
        return Ok(ExitCode::SUCCESS);
    }

    let env_targets = env_lifecycle::list_envs()
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?
        .into_iter()
        .filter(|(_, _, running)| *running)
        .map(|(name, _, _)| name)
        .collect::<Vec<_>>();
    for env_name in env_targets {
        close_env_and_wait(&env_name).await?;
    }

    let sessions = lifecycle::list_sessions()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let mut targets = sessions
        .into_iter()
        .filter(|(_, _, running, _)| *running)
        .map(|(name, _, _, _)| name)
        .collect::<Vec<_>>();
    targets.sort();

    for session_name in targets {
        if send_action_success(&session_name, Action::Close)
            .await
            .is_err()
            && let Some(pid) = lifecycle::read_pid(&session_name)
        {
            let _ = lifecycle::kill_pid(pid);
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn close_env_and_wait(env_name: &str) -> Result<(), CliError> {
    let request = Request {
        id: Uuid::new_v4(),
        action: Action::EnvClose {
            env: env_name.to_string(),
        },
    };
    let response = send_env_request(env_name, &request)
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    if !response.success {
        return Err(CliError::CommandFailed(response_error(&response)));
    }
    let env_socket = env_lifecycle::socket_path(env_name);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let running_instances = lifecycle::list_sessions()
            .await
            .map_err(|err| CliError::CommandFailed(err.to_string()))?
            .into_iter()
            .any(|(_, _, running, env)| running && env.as_deref() == Some(env_name));
        if !env_socket.exists() && !running_instances {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::CommandFailed(format!(
                "timed out waiting for env '{env_name}' to shut down"
            )));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

pub(crate) async fn fetch_signal_sample(
    session: &str,
    selector: &str,
) -> Result<(u64, u64, SignalValueData), CliError> {
    let response = send_action(
        session,
        Action::Sample {
            selectors: vec![selector.to_string()],
        },
    )
    .await?;
    if !response.success {
        return Err(CliError::CommandFailed(response_error(&response)));
    }
    match response.data {
        Some(ResponseData::SignalSample {
            tick,
            time_us,
            mut values,
        }) => {
            let value = values.drain(..).next().ok_or_else(|| {
                CliError::CommandFailed(format!("no values returned for '{selector}'"))
            })?;
            Ok((tick, time_us, value))
        }
        Some(other) => Err(CliError::CommandFailed(format!(
            "unexpected sample response payload: {other:?}"
        ))),
        None => Err(CliError::CommandFailed(
            "missing sample response payload".to_string(),
        )),
    }
}

pub(crate) async fn send_action_success(session: &str, action: Action) -> Result<(), CliError> {
    let response = send_action(session, action).await?;
    if response.success {
        Ok(())
    } else {
        Err(CliError::CommandFailed(response_error(&response)))
    }
}

pub(crate) async fn send_action(session: &str, action: Action) -> Result<Response, CliError> {
    let request = Request {
        id: Uuid::new_v4(),
        action,
    };
    send_request(session, &request)
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))
}

pub(crate) fn response_error(response: &Response) -> String {
    response
        .error
        .clone()
        .unwrap_or_else(|| "command failed".to_string())
}
