use super::{fetch_signal_sample, response_error, send_action, send_action_success};
use crate::cli::args::{CliArgs, RunArgs};
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::{
    AssertSpec, ForSpec, PrintSpec, RecipeStep, StepSpec, toml_value_to_cli_string,
};
use crate::daemon::lifecycle;
use crate::protocol::{
    Action, RecipeStepKindData, RecipeStepResultData, ResponseData, SignalValueData,
};
use crate::sim::types::SignalValue;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

enum RecipeOp {
    ForIteration {
        signal: String,
        value: f64,
    },
    Set {
        session: Option<String>,
        writes: BTreeMap<String, String>,
    },
    Step {
        session: Option<String>,
        duration: String,
    },
    Print {
        session: Option<String>,
        selectors: Vec<String>,
    },
    Speed {
        session: Option<String>,
        speed: f64,
    },
    Reset {
        session: Option<String>,
    },
    SleepMs(u64),
    Assert {
        session: Option<String>,
        assert: AssertSpec,
    },
}

pub(crate) async fn run_recipe_command(
    args: &CliArgs,
    run: &RunArgs,
) -> Result<ExitCode, CliError> {
    let config = load_config(args.config.as_deref().map(Path::new))
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let recipe_def = config
        .recipe(&run.recipe_name)
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;

    validate_recipe_preconditions(recipe_def).await?;

    let mut ops = Vec::new();
    compile_recipe_steps(&recipe_def.steps, &mut ops, None).map_err(CliError::CommandFailed)?;

    let default_session = recipe_def
        .session
        .as_deref()
        .unwrap_or(args.session.as_str())
        .to_string();
    let steps = execute_recipe_ops(&default_session, &ops, run.dry_run).await?;

    let response = crate::protocol::Response::ok(
        Uuid::new_v4(),
        ResponseData::RecipeResult {
            recipe: run.recipe_name.clone(),
            dry_run: run.dry_run,
            steps_executed: ops.len(),
            steps,
        },
    );
    crate::cli::output::print_response(&response, args.json);
    Ok(ExitCode::SUCCESS)
}

async fn validate_recipe_preconditions(
    recipe: &crate::config::recipe::RecipeDef,
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

fn compile_recipe_steps(
    steps: &[RecipeStep],
    ops: &mut Vec<RecipeOp>,
    inherited_session: Option<&str>,
) -> Result<(), String> {
    for step in steps {
        match step {
            RecipeStep::Set { set, session } => {
                let mut writes = BTreeMap::new();
                for (key, value) in set {
                    writes.insert(
                        key.clone(),
                        toml_value_to_cli_string(value).map_err(|e| e.to_string())?,
                    );
                }
                let session = session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Set { session, writes });
            }
            RecipeStep::Step { step, session } => {
                let (duration, nested_session) = match step {
                    StepSpec::Duration(duration) => (duration.clone(), None),
                    StepSpec::Detailed { duration, session } => (duration.clone(), session.clone()),
                };
                let session = nested_session
                    .or_else(|| session.clone())
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Step { session, duration });
            }
            RecipeStep::Print { print, session } => {
                let selectors = match print {
                    PrintSpec::All(value) if value == "*" => vec!["*".to_string()],
                    PrintSpec::All(value) => vec![value.clone()],
                    PrintSpec::Signals(values) => values.clone(),
                };
                let session = session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Print { session, selectors });
            }
            RecipeStep::Speed { speed, session } => {
                let session = session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Speed {
                    session,
                    speed: *speed,
                });
            }
            RecipeStep::Reset { session, .. } => {
                let session = session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Reset { session });
            }
            RecipeStep::Sleep { sleep: ms } => ops.push(RecipeOp::SleepMs(*ms)),
            RecipeStep::For { r#for, session } => {
                let session = session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                compile_for_step(r#for, ops, session.as_deref())?;
            }
            RecipeStep::Assert { assert } => {
                validate_assert_spec(&assert.signal, assert)?;
                let session = assert
                    .session
                    .clone()
                    .or_else(|| inherited_session.map(ToString::to_string));
                ops.push(RecipeOp::Assert {
                    session,
                    assert: assert.clone(),
                });
            }
        }
    }
    Ok(())
}

fn compile_for_step(
    spec: &ForSpec,
    ops: &mut Vec<RecipeOp>,
    inherited_session: Option<&str>,
) -> Result<(), String> {
    if spec.by == 0.0 {
        return Err("for.by cannot be zero".to_string());
    }
    let delta = spec.to - spec.from;
    if (spec.by > 0.0 && delta < 0.0) || (spec.by < 0.0 && delta > 0.0) {
        return Ok(());
    }
    let raw_steps = delta / spec.by;
    if !raw_steps.is_finite() {
        return Err("for range is not finite".to_string());
    }
    let epsilon = 1e-9_f64;
    let max_steps_float = (raw_steps + epsilon).floor();
    debug_assert!(max_steps_float >= 0.0);
    if max_steps_float >= u64::MAX as f64 {
        return Err("for range expands to too many iterations".to_string());
    }
    let max_steps = max_steps_float as u64;

    for idx in 0..=max_steps {
        let current = spec.from + spec.by * idx as f64;
        ops.push(RecipeOp::ForIteration {
            signal: spec.signal.clone(),
            value: current,
        });
        let mut writes = BTreeMap::new();
        writes.insert(spec.signal.clone(), current.to_string());
        ops.push(RecipeOp::Set {
            session: inherited_session.map(ToString::to_string),
            writes,
        });
        compile_recipe_steps(&spec.each, ops, inherited_session)?;
    }
    Ok(())
}

async fn execute_recipe_ops(
    default_session: &str,
    ops: &[RecipeOp],
    dry_run: bool,
) -> Result<Vec<RecipeStepResultData>, CliError> {
    let mut results = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            RecipeOp::ForIteration { signal, value } => {
                results.push(step_result(
                    RecipeStepKindData::ForIteration,
                    None,
                    format!("{signal}={value}"),
                ));
            }
            RecipeOp::Set { session, writes } => {
                let session_name = resolve_session(default_session, session);
                if !dry_run {
                    send_action_success(
                        &session_name,
                        Action::Set {
                            writes: writes.clone(),
                        },
                    )
                    .await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Set,
                    Some(session_name),
                    format!(
                        "{} write(s): {}",
                        writes.len(),
                        writes.keys().cloned().collect::<Vec<_>>().join(",")
                    ),
                ));
            }
            RecipeOp::Step { session, duration } => {
                let session_name = resolve_session(default_session, session);
                if !dry_run {
                    send_action_success(
                        &session_name,
                        Action::TimeStep {
                            duration: duration.clone(),
                        },
                    )
                    .await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Step,
                    Some(session_name),
                    duration.clone(),
                ));
            }
            RecipeOp::Print { session, selectors } => {
                let session_name = resolve_session(default_session, session);
                let detail = if dry_run {
                    format!("selectors={}", selectors.join(","))
                } else {
                    let values = fetch_print_values(&session_name, selectors).await?;
                    format_signal_values_summary(&values)
                };
                results.push(step_result(
                    RecipeStepKindData::Print,
                    Some(session_name),
                    detail,
                ));
            }
            RecipeOp::Speed { session, speed } => {
                let session_name = resolve_session(default_session, session);
                if !dry_run {
                    send_action_success(
                        &session_name,
                        Action::TimeSpeed {
                            multiplier: Some(*speed),
                        },
                    )
                    .await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Speed,
                    Some(session_name),
                    speed.to_string(),
                ));
            }
            RecipeOp::Reset { session } => {
                let session_name = resolve_session(default_session, session);
                if !dry_run {
                    send_action_success(&session_name, Action::Reset).await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Reset,
                    Some(session_name),
                    "reset".to_string(),
                ));
            }
            RecipeOp::SleepMs(ms) => {
                if !dry_run {
                    sleep(Duration::from_millis(*ms)).await;
                }
                results.push(step_result(
                    RecipeStepKindData::Sleep,
                    None,
                    format!("{ms}ms"),
                ));
            }
            RecipeOp::Assert { session, assert } => {
                let session_name = resolve_session(default_session, session);
                if dry_run {
                    results.push(step_result(
                        RecipeStepKindData::Assert,
                        Some(session_name),
                        format!("signal={}", assert.signal),
                    ));
                    continue;
                }
                let (tick, time_us, value) =
                    fetch_signal_sample(&session_name, &assert.signal).await?;
                evaluate_assertion(&assert.signal, &value.value, assert).map_err(|message| {
                    CliError::AssertionFailed(format!("{message}; tick={tick} time_us={time_us}"))
                })?;
                results.push(step_result(
                    RecipeStepKindData::Assert,
                    Some(session_name),
                    format!("{} @ tick={} time_us={}", assert.signal, tick, time_us),
                ));
            }
        }
    }
    Ok(results)
}

async fn fetch_print_values(
    session: &str,
    selectors: &[String],
) -> Result<Vec<SignalValueData>, CliError> {
    let response = send_action(
        session,
        Action::Get {
            selectors: selectors.to_vec(),
        },
    )
    .await?;
    if !response.success {
        return Err(CliError::CommandFailed(response_error(&response)));
    }
    match response.data {
        Some(ResponseData::SignalValues { values }) => Ok(values),
        Some(other) => Err(CliError::CommandFailed(format!(
            "unexpected get response payload: {other:?}"
        ))),
        None => Err(CliError::CommandFailed(
            "missing get response payload".to_string(),
        )),
    }
}

fn format_signal_values_summary(values: &[SignalValueData]) -> String {
    values
        .iter()
        .map(|value| format!("{}={:?}", value.name, value.value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn step_result(
    kind: RecipeStepKindData,
    session: Option<String>,
    detail: String,
) -> RecipeStepResultData {
    RecipeStepResultData {
        kind,
        session,
        detail,
    }
}

fn resolve_session(default_session: &str, session: &Option<String>) -> String {
    session.as_deref().unwrap_or(default_session).to_string()
}

fn validate_assert_spec(signal: &str, assert: &AssertSpec) -> Result<(), String> {
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
    if assert.approx.is_some() && assert.tolerance.is_none() {
        return Err(format!(
            "assert approx for '{signal}' must define an explicit tolerance"
        ));
    }
    if assert.approx.is_none() && assert.tolerance.is_some() {
        return Err(format!(
            "assert tolerance for '{signal}' is only valid with approx"
        ));
    }
    Ok(())
}

fn evaluate_assertion(
    signal: &str,
    actual: &SignalValue,
    assert: &AssertSpec,
) -> Result<(), String> {
    validate_assert_spec(signal, assert)?;

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
        let tolerance = assert
            .tolerance
            .ok_or_else(|| {
                format!("assert approx for '{signal}' must define an explicit tolerance")
            })?
            .abs();
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

#[cfg(test)]
mod tests {
    use super::{RecipeOp, compile_for_step, validate_assert_spec};
    use crate::config::recipe::{AssertSpec, ForSpec};

    #[test]
    fn compile_for_step_uses_stable_iteration_count_for_fractional_steps() {
        let spec = ForSpec {
            signal: "demo.input".to_string(),
            from: 0.0,
            to: 1.0,
            by: 0.1,
            each: Vec::new(),
        };
        let mut ops = Vec::new();
        compile_for_step(&spec, &mut ops, None).expect("for-step compile should succeed");
        assert_eq!(ops.len(), 22);
        for (idx, op) in ops
            .iter()
            .filter_map(|op| match op {
                RecipeOp::Set { writes, .. } => Some(writes),
                _ => None,
            })
            .enumerate()
        {
            let raw = op
                .get("demo.input")
                .expect("compiled write should include loop signal");
            let value = raw
                .parse::<f64>()
                .expect("compiled write value should parse as f64");
            let expected = idx as f64 * 0.1;
            assert!(
                (value - expected).abs() <= 1e-9,
                "expected value {expected}, got {value}"
            );
        }
    }

    #[test]
    fn compile_for_step_rejects_u64_max_edge_case() {
        let spec = ForSpec {
            signal: "demo.input".to_string(),
            from: 0.0,
            to: u64::MAX as f64,
            by: 1.0,
            each: Vec::new(),
        };
        let mut ops = Vec::new();
        let err =
            compile_for_step(&spec, &mut ops, None).expect_err("2^64-sized range must be rejected");
        assert!(
            err.contains("too many iterations"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn approx_requires_explicit_tolerance() {
        let err = validate_assert_spec(
            "demo.output",
            &AssertSpec {
                signal: "demo.output".to_string(),
                session: None,
                eq: None,
                gt: None,
                lt: None,
                gte: None,
                lte: None,
                approx: Some(1.0),
                tolerance: None,
            },
        )
        .expect_err("approx without tolerance should fail");
        assert!(err.contains("explicit tolerance"));
    }
}
