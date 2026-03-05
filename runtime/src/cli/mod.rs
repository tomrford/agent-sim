pub mod args;
pub mod commands;
mod env;
pub mod error;
pub mod output;
mod recipe;

use crate::cli::args::{CliArgs, CloseArgs, Command, WatchArgs};
use crate::cli::commands::to_request;
use crate::cli::error::CliError;
use crate::connection::send_request;
use crate::daemon::lifecycle;
use crate::protocol::{Action, Request, Response, ResponseData, SignalValueData, WatchSampleData};
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
        Command::Watch(watch) => run_watch_command(&args, watch).await,
        Command::Run(run) => recipe::run_recipe_command(&args, run).await,
        Command::Close(close) if close.all || close.env.is_some() => run_close_command(close).await,
        Command::Env(env) => env::run_env_command(&args, env).await,
        _ => {
            let request = to_request(&args)?;
            let response = send_request(&args.session, &request)
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

async fn run_watch_command(args: &CliArgs, watch: &WatchArgs) -> Result<ExitCode, CliError> {
    let count = watch.samples.unwrap_or(10).max(1);
    let mut samples = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let (tick, time_us, signal_value) =
            fetch_signal_sample(&args.session, &watch.selector).await?;
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
    let sessions = lifecycle::list_sessions()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let mut targets = sessions
        .into_iter()
        .filter(|(_, _, running, _)| *running)
        .filter(|(_, _, _, env)| match &close.env {
            Some(requested_env) => env.as_ref() == Some(requested_env),
            None => true,
        })
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
