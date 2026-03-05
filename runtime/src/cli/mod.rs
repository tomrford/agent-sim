pub mod args;
pub mod commands;
pub mod error;
pub mod output;

use crate::cli::args::{CliArgs, CloseArgs, Command, EnvArgs, EnvCommand, RunArgs, WatchArgs};
use crate::cli::commands::to_request;
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::{
    AssertSpec, EnvCanBus, EnvDef, ForSpec, PrintSpec, RecipeStep, toml_value_to_cli_string,
};
use crate::connection::send_request;
use crate::daemon::lifecycle;
use crate::protocol::{Action, Request, Response, ResponseData, SignalValueData, WatchSampleData};
use crate::sim::types::SignalValue;
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
        Command::Close(close) if close.all || close.env.is_some() => {
            run_close_command(&args, close).await
        }
        Command::Env(env) => run_env_command(&args, env).await,
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
    Assert(AssertSpec),
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

    validate_recipe_preconditions(recipe_def, &config, args).await?;

    let mut events = Vec::new();
    let mut ops = Vec::new();
    compile_recipe_steps(&recipe_def.steps, &mut ops, &mut events)
        .map_err(CliError::CommandFailed)?;

    let target_session = recipe_def
        .session
        .as_deref()
        .unwrap_or(args.session.as_str())
        .to_string();
    if !run.dry_run {
        execute_recipe_ops(&target_session, &ops).await?;
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

async fn run_close_command(_args: &CliArgs, close: &CloseArgs) -> Result<ExitCode, CliError> {
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

async fn run_env_command(args: &CliArgs, env: &EnvArgs) -> Result<ExitCode, CliError> {
    match &env.command {
        EnvCommand::Start { name } => run_env_start(args, name).await,
    }
}

async fn run_env_start(args: &CliArgs, env_name: &str) -> Result<ExitCode, CliError> {
    let config = load_config(args.config.as_deref().map(Path::new))
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let env_def = config
        .env(env_name)
        .map_err(|e| CliError::CommandFailed(e.to_string()))?
        .clone();
    validate_env_can_ifaces(&env_def)?;
    ensure_sessions_available(&env_def).await?;

    let mut started_sessions = Vec::new();
    let result = start_env_internal(env_name, &env_def, &mut started_sessions).await;
    if let Err(err) = result {
        rollback_started_sessions(&started_sessions).await;
        return Err(err);
    }
    Ok(ExitCode::SUCCESS)
}

async fn validate_recipe_preconditions(
    recipe: &crate::config::recipe::RecipeDef,
    _config: &crate::config::AppConfig,
    _args: &CliArgs,
) -> Result<(), CliError> {
    let sessions = lifecycle::list_sessions()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let running = sessions
        .iter()
        .filter(|(_, _, is_running, _)| *is_running)
        .map(|(name, _, _, env)| (name.clone(), env.clone()))
        .collect::<Vec<_>>();

    if let Some(env_name) = &recipe.env {
        let has_env = running
            .iter()
            .any(|(_, env)| env.as_ref() == Some(env_name));
        if !has_env {
            return Err(CliError::CommandFailed(format!(
                "recipe requires env '{env_name}', but no matching running sessions were found"
            )));
        }
    }

    for session_name in &recipe.sessions {
        let is_running = running.iter().any(|(name, _)| name == session_name);
        if !is_running {
            return Err(CliError::CommandFailed(format!(
                "recipe requires session '{session_name}' to be running"
            )));
        }
    }
    Ok(())
}

fn validate_env_can_ifaces(env_def: &EnvDef) -> Result<(), CliError> {
    #[cfg(target_os = "linux")]
    {
        for bus in env_def.can.values() {
            let iface_path = Path::new("/sys/class/net").join(&bus.vcan);
            if !iface_path.exists() {
                return Err(CliError::CommandFailed(format!(
                    "VCAN interface '{}' does not exist",
                    bus.vcan
                )));
            }
            let operstate_path = iface_path.join("operstate");
            let state = std::fs::read_to_string(&operstate_path)
                .unwrap_or_else(|_| "unknown".to_string())
                .trim()
                .to_string();
            if state != "up" && state != "unknown" {
                return Err(CliError::CommandFailed(format!(
                    "VCAN interface '{}' is not up (state: {state})",
                    bus.vcan
                )));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = env_def;
    }
    Ok(())
}

async fn ensure_sessions_available(env_def: &EnvDef) -> Result<(), CliError> {
    let sessions = lifecycle::list_sessions()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    for session in &env_def.sessions {
        if sessions
            .iter()
            .any(|(name, _, running, _)| name == &session.name && *running)
        {
            return Err(CliError::CommandFailed(format!(
                "session '{}' is already running",
                session.name
            )));
        }
    }
    Ok(())
}

async fn start_env_internal(
    env_name: &str,
    env_def: &EnvDef,
    started_sessions: &mut Vec<String>,
) -> Result<(), CliError> {
    for session in &env_def.sessions {
        let response = send_action(
            &session.name,
            Action::Load {
                libpath: session.lib.clone(),
                env_tag: Some(env_name.to_string()),
            },
        )
        .await?;
        if !response.success {
            return Err(CliError::CommandFailed(response_error(&response)));
        }
        started_sessions.push(session.name.clone());
    }

    for (bus_name, bus) in &env_def.can {
        apply_env_bus_wiring(bus_name, bus).await?;
    }
    Ok(())
}

async fn apply_env_bus_wiring(bus_name: &str, bus: &EnvCanBus) -> Result<(), CliError> {
    for member in &bus.members {
        let (session_name, member_bus_name) = parse_env_member(member, bus_name)?;
        send_action_success(
            &session_name,
            Action::CanAttach {
                bus_name: member_bus_name.clone(),
                vcan_iface: bus.vcan.clone(),
            },
        )
        .await?;
        if let Some(dbc_path) = &bus.dbc {
            send_action_success(
                &session_name,
                Action::CanLoadDbc {
                    bus_name: member_bus_name,
                    path: dbc_path.clone(),
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn parse_env_member(member: &str, default_bus_name: &str) -> Result<(String, String), CliError> {
    if let Some((session_name, bus_name)) = member.split_once(':') {
        if session_name.is_empty() || bus_name.is_empty() {
            return Err(CliError::CommandFailed(format!(
                "invalid env bus member '{member}'"
            )));
        }
        return Ok((session_name.to_string(), bus_name.to_string()));
    }
    if member.is_empty() {
        return Err(CliError::CommandFailed("empty env bus member".to_string()));
    }
    Ok((member.to_string(), default_bus_name.to_string()))
}

async fn rollback_started_sessions(started_sessions: &[String]) {
    for session_name in started_sessions {
        if send_action_success(session_name, Action::Close)
            .await
            .is_err()
            && let Some(pid) = lifecycle::read_pid(session_name)
        {
            let _ = lifecycle::kill_pid(pid);
        }
    }
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
            RecipeStep::Assert { assert } => {
                events.push(format!("assert {}", assert.signal));
                ops.push(RecipeOp::Assert(assert.clone()));
            }
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
            RecipeOp::Assert(assert) => {
                let value = fetch_first_signal_value(session, &assert.signal).await?;
                let context = format_tick_context(session)
                    .await
                    .unwrap_or_else(|| "time=unknown".to_string());
                evaluate_assertion(&assert.signal, &value.value, assert).map_err(|message| {
                    CliError::AssertionFailed(format!("{message}; {context}"))
                })?;
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

async fn format_tick_context(session: &str) -> Option<String> {
    fetch_time_snapshot(session)
        .await
        .ok()
        .map(|(ticks, time_us)| format!("tick={ticks} time_us={time_us}"))
}

fn evaluate_assertion(
    signal: &str,
    actual: &SignalValue,
    assert: &AssertSpec,
) -> Result<(), String> {
    let comparator_count = [
        assert.eq.is_some(),
        assert.gt.is_some(),
        assert.lt.is_some(),
        assert.gte.is_some(),
        assert.lte.is_some(),
        assert.approx.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if comparator_count == 0 {
        return Err(format!(
            "assert step for '{signal}' must define one comparator (eq/gt/lt/gte/lte/approx)"
        ));
    }
    if comparator_count > 1 {
        return Err(format!(
            "assert step for '{signal}' defines multiple comparators; use exactly one"
        ));
    }

    if let Some(expected) = &assert.eq {
        let ok = compare_eq(actual, expected)?;
        if !ok {
            return Err(format!(
                "assert eq failed for '{signal}': expected {expected:?}, got {actual:?}"
            ));
        }
        return Ok(());
    }

    let actual_num = signal_value_as_f64(actual)
        .ok_or_else(|| format!("assertion for '{signal}' expects numeric value, got {actual:?}"))?;

    if let Some(expected) = assert.gt {
        if actual_num > expected {
            return Ok(());
        }
        return Err(format!(
            "assert gt failed for '{signal}': expected > {expected}, got {actual_num}"
        ));
    }
    if let Some(expected) = assert.lt {
        if actual_num < expected {
            return Ok(());
        }
        return Err(format!(
            "assert lt failed for '{signal}': expected < {expected}, got {actual_num}"
        ));
    }
    if let Some(expected) = assert.gte {
        if actual_num >= expected {
            return Ok(());
        }
        return Err(format!(
            "assert gte failed for '{signal}': expected >= {expected}, got {actual_num}"
        ));
    }
    if let Some(expected) = assert.lte {
        if actual_num <= expected {
            return Ok(());
        }
        return Err(format!(
            "assert lte failed for '{signal}': expected <= {expected}, got {actual_num}"
        ));
    }
    if let Some(expected) = assert.approx {
        let tolerance = assert.tolerance.unwrap_or(0.0).abs();
        if (actual_num - expected).abs() <= tolerance {
            return Ok(());
        }
        return Err(format!(
            "assert approx failed for '{signal}': expected {expected} ± {tolerance}, got {actual_num}"
        ));
    }

    Err(format!("assertion for '{signal}' is invalid"))
}

fn compare_eq(actual: &SignalValue, expected: &toml::Value) -> Result<bool, String> {
    match (actual, expected) {
        (SignalValue::Bool(a), toml::Value::Boolean(b)) => Ok(*a == *b),
        (SignalValue::U32(a), toml::Value::Integer(b)) => Ok((*a as i64) == *b),
        (SignalValue::I32(a), toml::Value::Integer(b)) => Ok((*a as i64) == *b),
        (SignalValue::F32(a), toml::Value::Float(b)) => Ok((*a as f64) == *b),
        (SignalValue::F64(a), toml::Value::Float(b)) => Ok(*a == *b),
        (_, toml::Value::Float(b)) => signal_value_as_f64(actual)
            .map(|a| a == *b)
            .ok_or_else(|| format!("cannot compare non-numeric value {actual:?} to float {b}")),
        (_, toml::Value::Integer(b)) => signal_value_as_f64(actual)
            .map(|a| (a - (*b as f64)).abs() < f64::EPSILON)
            .ok_or_else(|| format!("cannot compare non-numeric value {actual:?} to integer {b}")),
        _ => Err(format!(
            "unsupported eq comparator type for value {actual:?}: expected {expected:?}"
        )),
    }
}

fn signal_value_as_f64(value: &SignalValue) -> Option<f64> {
    match value {
        SignalValue::Bool(v) => Some(if *v { 1.0 } else { 0.0 }),
        SignalValue::U32(v) => Some(*v as f64),
        SignalValue::I32(v) => Some(*v as f64),
        SignalValue::F32(v) => Some(*v as f64),
        SignalValue::F64(v) => Some(*v),
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
