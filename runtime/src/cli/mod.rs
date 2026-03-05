pub mod args;
pub mod commands;
pub mod error;
pub mod output;

use crate::cli::args::{CliArgs, Command, RunArgs, WatchArgs};
use crate::cli::commands::to_request;
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::{ForSpec, PrintSpec, RecipeStep, toml_value_to_cli_string};
use crate::connection::send_request;
use crate::protocol::{Action, Request, Response, ResponseData, SignalValueData, WatchSampleData};
use std::collections::BTreeMap;
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
        Command::Watch(watch) => run_watch_command(&args, watch).await,
        Command::Run(run) => run_recipe_command(&args, run).await,
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

enum RecipeOp {
    Set(BTreeMap<String, String>),
    Step(String),
    Print(Vec<String>),
    Speed(f64),
    Reset,
    SleepMs(u64),
}

async fn run_watch_command(args: &CliArgs, watch: &WatchArgs) -> Result<ExitCode, CliError> {
    let count = watch.samples.unwrap_or(10).max(1);
    let mut samples = Vec::with_capacity(count as usize);
    for idx in 0..count {
        let signal_value = fetch_first_signal_value(&args.session, &watch.selector).await?;
        let (tick, time_us) = fetch_time_snapshot(&args.session).await?;
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

async fn run_recipe_command(args: &CliArgs, run: &RunArgs) -> Result<ExitCode, CliError> {
    let config = load_config(args.config.as_deref().map(Path::new))
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let recipe_def = config
        .recipe(&run.recipe_name)
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;

    let mut events = Vec::new();
    let mut ops = Vec::new();
    compile_recipe_steps(&recipe_def.steps, &mut ops, &mut events)
        .map_err(CliError::CommandFailed)?;

    if !run.dry_run {
        execute_recipe_ops(&args.session, &ops).await?;
    }

    let response = Response::ok(
        Uuid::new_v4(),
        ResponseData::RecipeResult {
            recipe: run.recipe_name.clone(),
            dry_run: run.dry_run,
            steps_executed: recipe_def.steps.len(),
            events,
        },
    );
    output::print_response(&response, args.json);
    Ok(ExitCode::SUCCESS)
}

fn compile_recipe_steps(
    steps: &[RecipeStep],
    ops: &mut Vec<RecipeOp>,
    events: &mut Vec<String>,
) -> Result<(), String> {
    for step in steps {
        match step {
            RecipeStep::Set { set } => {
                let mut writes = BTreeMap::new();
                for (key, value) in set {
                    writes.insert(
                        key.clone(),
                        toml_value_to_cli_string(value).map_err(|e| e.to_string())?,
                    );
                }
                events.push(format!("set {}", writes.len()));
                ops.push(RecipeOp::Set(writes));
            }
            RecipeStep::Step { step } => {
                events.push(format!("step {step}"));
                ops.push(RecipeOp::Step(step.clone()));
            }
            RecipeStep::Print { print } => {
                let selectors = match print {
                    PrintSpec::All(value) if value == "*" => vec!["*".to_string()],
                    PrintSpec::All(value) => vec![value.clone()],
                    PrintSpec::Signals(values) => values.clone(),
                };
                events.push(format!("print {}", selectors.join(",")));
                ops.push(RecipeOp::Print(selectors));
            }
            RecipeStep::Speed { speed } => {
                events.push(format!("speed {speed}"));
                ops.push(RecipeOp::Speed(*speed));
            }
            RecipeStep::Reset { .. } => {
                events.push("reset".to_string());
                ops.push(RecipeOp::Reset);
            }
            RecipeStep::Sleep { sleep: ms } => {
                events.push(format!("sleep {ms}ms"));
                ops.push(RecipeOp::SleepMs(*ms));
            }
            RecipeStep::For { r#for } => compile_for_step(r#for, ops, events)?,
        }
    }
    Ok(())
}

fn compile_for_step(
    spec: &ForSpec,
    ops: &mut Vec<RecipeOp>,
    events: &mut Vec<String>,
) -> Result<(), String> {
    if spec.by == 0.0 {
        return Err("for.by cannot be zero".to_string());
    }
    let mut current = spec.from;
    let within_bounds = |v: f64| {
        if spec.by > 0.0 {
            v <= spec.to
        } else {
            v >= spec.to
        }
    };
    while within_bounds(current) {
        events.push(format!("for {}={current}", spec.signal));
        let mut writes = BTreeMap::new();
        writes.insert(spec.signal.clone(), current.to_string());
        ops.push(RecipeOp::Set(writes));
        compile_recipe_steps(&spec.each, ops, events)?;
        current += spec.by;
    }
    Ok(())
}

async fn execute_recipe_ops(session: &str, ops: &[RecipeOp]) -> Result<(), CliError> {
    for op in ops {
        match op {
            RecipeOp::Set(writes) => {
                send_action_success(
                    session,
                    Action::Set {
                        writes: writes.clone(),
                    },
                )
                .await?;
            }
            RecipeOp::Step(duration) => {
                send_action_success(
                    session,
                    Action::TimeStep {
                        duration: duration.clone(),
                    },
                )
                .await?;
            }
            RecipeOp::Print(selectors) => {
                send_action_success(
                    session,
                    Action::Get {
                        selectors: selectors.clone(),
                    },
                )
                .await?;
            }
            RecipeOp::Speed(speed) => {
                send_action_success(
                    session,
                    Action::TimeSpeed {
                        multiplier: Some(*speed),
                    },
                )
                .await?;
            }
            RecipeOp::Reset => {
                send_action_success(session, Action::Reset).await?;
            }
            RecipeOp::SleepMs(ms) => {
                sleep(Duration::from_millis(*ms)).await;
            }
        }
    }
    Ok(())
}

async fn fetch_first_signal_value(
    session: &str,
    selector: &str,
) -> Result<SignalValueData, CliError> {
    let response = send_action(
        session,
        Action::Get {
            selectors: vec![selector.to_string()],
        },
    )
    .await?;
    if !response.success {
        return Err(CliError::CommandFailed(response_error(&response)));
    }
    match response.data {
        Some(ResponseData::SignalValues { values }) => values
            .into_iter()
            .next()
            .ok_or_else(|| CliError::CommandFailed(format!("no values returned for '{selector}'"))),
        Some(other) => Err(CliError::CommandFailed(format!(
            "unexpected get response payload: {other:?}"
        ))),
        None => Err(CliError::CommandFailed(
            "missing get response payload".to_string(),
        )),
    }
}

async fn fetch_time_snapshot(session: &str) -> Result<(u64, u64), CliError> {
    let response = send_action(session, Action::TimeStatus).await?;
    if !response.success {
        return Err(CliError::CommandFailed(response_error(&response)));
    }
    match response.data {
        Some(ResponseData::TimeStatus {
            elapsed_ticks,
            elapsed_time_us,
            ..
        }) => Ok((elapsed_ticks, elapsed_time_us)),
        Some(other) => Err(CliError::CommandFailed(format!(
            "unexpected time status payload: {other:?}"
        ))),
        None => Err(CliError::CommandFailed(
            "missing time status response payload".to_string(),
        )),
    }
}

async fn send_action_success(session: &str, action: Action) -> Result<(), CliError> {
    let response = send_action(session, action).await?;
    if response.success {
        Ok(())
    } else {
        Err(CliError::CommandFailed(response_error(&response)))
    }
}

async fn send_action(session: &str, action: Action) -> Result<Response, CliError> {
    let request = Request {
        id: Uuid::new_v4(),
        action,
    };
    send_request(session, &request)
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))
}

fn response_error(response: &Response) -> String {
    response
        .error
        .clone()
        .unwrap_or_else(|| "command failed".to_string())
}
