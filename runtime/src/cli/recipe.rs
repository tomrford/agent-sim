use super::{fetch_signal_sample, response_error, send_action, send_action_success};
use crate::cli::args::{CliArgs, RunArgs};
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::{
    AssertSpec, ForSpec, PrintSpec, RecipeStep, StepSpec, toml_value_to_cli_string,
};
use crate::connection::send_env_request;
use crate::daemon::lifecycle;
use crate::protocol::{
    EnvAction, InstanceAction, RequestAction, RecipeStepKindData, RecipeStepResultData,
    ResponseData, SignalValueData,
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
        instance: Option<String>,
        writes: BTreeMap<String, String>,
    },
    Step {
        instance: Option<String>,
        duration: String,
    },
    Print {
        instance: Option<String>,
        selectors: Vec<String>,
    },
    Speed {
        instance: Option<String>,
        speed: f64,
    },
    Reset {
        instance: Option<String>,
    },
    SleepMs(u64),
    Assert {
        instance: Option<String>,
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

    let default_instance = recipe_def
        .instance
        .as_deref()
        .unwrap_or(args.instance.as_str())
        .to_string();
    validate_recipe_preconditions(recipe_def, &default_instance).await?;

    let mut ops = Vec::new();
    compile_recipe_steps(&recipe_def.steps, &mut ops, None).map_err(CliError::CommandFailed)?;
    let steps = execute_recipe_ops(&default_instance, &ops, run.dry_run).await?;

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
    default_instance: &str,
) -> Result<(), CliError> {
    let instances = lifecycle::list_instances()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let running = instances
        .iter()
        .filter(|instance| instance.running)
        .map(|instance| (instance.name.clone(), instance.env.clone()))
        .collect::<Vec<_>>();

    let default_instance_env = running
        .iter()
        .find(|(name, _)| name == default_instance)
        .map(|(_, env)| env.as_deref());
    if default_instance_env.is_none() {
        return Err(CliError::CommandFailed(format!(
            "recipe target instance '{default_instance}' is not running"
        )));
    }

    if !recipe.env.is_empty() {
        validate_allowed_env(
            default_instance,
            default_instance_env.flatten(),
            &recipe.env,
        )?;
    }

    for instance_name in &recipe.instances {
        let instance_env = running
            .iter()
            .find(|(name, _)| name == instance_name)
            .map(|(_, env)| env.as_deref());
        if instance_env.is_none() {
            return Err(CliError::CommandFailed(format!(
                "recipe requires instance '{instance_name}' to be running"
            )));
        }
        if !recipe.env.is_empty() {
            validate_allowed_env(instance_name, instance_env.flatten(), &recipe.env)?;
        }
    }
    Ok(())
}

fn validate_allowed_env(
    instance_name: &str,
    instance_env: Option<&str>,
    allowed_envs: &[String],
) -> Result<(), CliError> {
    let allowed = allowed_envs.join(", ");
    match instance_env {
        Some(env_name)
            if allowed_envs
                .iter()
                .any(|allowed_env| allowed_env == env_name) =>
        {
            Ok(())
        }
        Some(env_name) => Err(CliError::CommandFailed(format!(
            "recipe only allowed in envs [{allowed}], but instance '{instance_name}' is in env '{env_name}'"
        ))),
        None => Err(CliError::CommandFailed(format!(
            "recipe only allowed in envs [{allowed}], but instance '{instance_name}' is not attached to any env"
        ))),
    }
}

fn compile_recipe_steps(
    steps: &[RecipeStep],
    ops: &mut Vec<RecipeOp>,
    inherited_instance: Option<&str>,
) -> Result<(), String> {
    for step in steps {
        match step {
            RecipeStep::Set { set, instance } => {
                let mut writes = BTreeMap::new();
                for (key, value) in set {
                    writes.insert(
                        key.clone(),
                        toml_value_to_cli_string(value).map_err(|e| e.to_string())?,
                    );
                }
                let instance = instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Set { instance, writes });
            }
            RecipeStep::Step { step, instance } => {
                let (duration, nested_instance) = match step {
                    StepSpec::Duration(duration) => (duration.clone(), None),
                    StepSpec::Detailed { duration, instance } => {
                        (duration.clone(), instance.clone())
                    }
                };
                let instance = nested_instance
                    .or_else(|| instance.clone())
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Step { instance, duration });
            }
            RecipeStep::Print { print, instance } => {
                let selectors = match print {
                    PrintSpec::All(value) if value == "*" => vec!["*".to_string()],
                    PrintSpec::All(value) => vec![value.clone()],
                    PrintSpec::Signals(values) => values.clone(),
                };
                let instance = instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Print {
                    instance,
                    selectors,
                });
            }
            RecipeStep::Speed { speed, instance } => {
                let instance = instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Speed {
                    instance,
                    speed: *speed,
                });
            }
            RecipeStep::Reset { instance, .. } => {
                let instance = instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Reset { instance });
            }
            RecipeStep::Sleep { sleep: ms } => ops.push(RecipeOp::SleepMs(*ms)),
            RecipeStep::For { r#for, instance } => {
                let instance = instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                compile_for_step(r#for, ops, instance.as_deref())?;
            }
            RecipeStep::Assert { assert } => {
                validate_assert_spec(&assert.signal, assert)?;
                let instance = assert
                    .instance
                    .clone()
                    .or_else(|| inherited_instance.map(ToString::to_string));
                ops.push(RecipeOp::Assert {
                    instance,
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
    inherited_instance: Option<&str>,
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
            instance: inherited_instance.map(ToString::to_string),
            writes,
        });
        compile_recipe_steps(&spec.each, ops, inherited_instance)?;
    }
    Ok(())
}

async fn execute_recipe_ops(
    default_instance: &str,
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
            RecipeOp::Set { instance, writes } => {
                let instance_name = resolve_instance(default_instance, instance);
                if !dry_run {
                    send_action_success(
                        &instance_name,
                        InstanceAction::Set {
                            writes: writes.clone(),
                        },
                    )
                    .await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Set,
                    Some(instance_name),
                    format!(
                        "{} write(s): {}",
                        writes.len(),
                        writes.keys().cloned().collect::<Vec<_>>().join(",")
                    ),
                ));
            }
            RecipeOp::Step { instance, duration } => {
                let instance_name = resolve_instance(default_instance, instance);
                if !dry_run {
                    if let Some(env_name) = attached_env_name(&instance_name).await? {
                        send_env_action_success(
                            &env_name,
                            EnvAction::TimeStep {
                                env: env_name.clone(),
                                duration: duration.clone(),
                            },
                        )
                        .await?;
                    } else {
                        send_action_success(
                            &instance_name,
                            InstanceAction::TimeStep {
                                duration: duration.clone(),
                            },
                        )
                        .await?;
                    }
                }
                results.push(step_result(
                    RecipeStepKindData::Step,
                    Some(instance_name),
                    duration.clone(),
                ));
            }
            RecipeOp::Print {
                instance,
                selectors,
            } => {
                let instance_name = resolve_instance(default_instance, instance);
                let detail = if dry_run {
                    format!("selectors={}", selectors.join(","))
                } else {
                    let values = fetch_print_values(&instance_name, selectors).await?;
                    format_signal_values_summary(&values)
                };
                results.push(step_result(
                    RecipeStepKindData::Print,
                    Some(instance_name),
                    detail,
                ));
            }
            RecipeOp::Speed { instance, speed } => {
                let instance_name = resolve_instance(default_instance, instance);
                if !dry_run {
                    if let Some(env_name) = attached_env_name(&instance_name).await? {
                        send_env_action_success(
                            &env_name,
                            EnvAction::TimeSpeed {
                                env: env_name.clone(),
                                multiplier: Some(*speed),
                            },
                        )
                        .await?;
                    } else {
                        send_action_success(
                            &instance_name,
                            InstanceAction::TimeSpeed {
                                multiplier: Some(*speed),
                            },
                        )
                        .await?;
                    }
                }
                results.push(step_result(
                    RecipeStepKindData::Speed,
                    Some(instance_name),
                    speed.to_string(),
                ));
            }
            RecipeOp::Reset { instance } => {
                let instance_name = resolve_instance(default_instance, instance);
                if !dry_run {
                    send_action_success(&instance_name, InstanceAction::Reset).await?;
                }
                results.push(step_result(
                    RecipeStepKindData::Reset,
                    Some(instance_name),
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
            RecipeOp::Assert { instance, assert } => {
                let instance_name = resolve_instance(default_instance, instance);
                if dry_run {
                    results.push(step_result(
                        RecipeStepKindData::Assert,
                        Some(instance_name),
                        format!("signal={}", assert.signal),
                    ));
                    continue;
                }
                let (tick, time_us, value) =
                    fetch_signal_sample(&instance_name, &assert.signal).await?;
                evaluate_assertion(&assert.signal, &value.value, assert).map_err(|message| {
                    CliError::AssertionFailed(format!("{message}; tick={tick} time_us={time_us}"))
                })?;
                results.push(step_result(
                    RecipeStepKindData::Assert,
                    Some(instance_name),
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
        InstanceAction::Get {
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
    instance: Option<String>,
    detail: String,
) -> RecipeStepResultData {
    RecipeStepResultData {
        kind,
        instance,
        detail,
    }
}

fn resolve_instance(default_instance: &str, instance: &Option<String>) -> String {
    instance.as_deref().unwrap_or(default_instance).to_string()
}

async fn attached_env_name(instance: &str) -> Result<Option<String>, CliError> {
    let running = lifecycle::list_instances()
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    Ok(running
        .into_iter()
        .find(|running_instance| running_instance.name == instance && running_instance.running)
        .and_then(|running_instance| running_instance.env))
}

async fn send_env_action_success(env: &str, action: EnvAction) -> Result<(), CliError> {
    let response = send_env_request(
        env,
        &crate::protocol::Request {
            id: Uuid::new_v4(),
            action: RequestAction::Env(action),
        },
    )
    .await
    .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    if response.success {
        Ok(())
    } else {
        Err(CliError::CommandFailed(response_error(&response)))
    }
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
        (SignalValue::F32(a), toml::Value::Float(b)) => Ok(*a == (*b as f32)),
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
    use super::{RecipeOp, compare_eq, compile_for_step, validate_assert_spec};
    use crate::config::recipe::{AssertSpec, ForSpec};
    use crate::sim::types::SignalValue;

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
                instance: None,
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

    #[test]
    fn compare_eq_f32_decimal_literal_matches_f32_value() {
        let actual = SignalValue::F32(0.1_f32);
        let expected = toml::Value::Float(0.1_f64);
        let equal = compare_eq(&actual, &expected).expect("f32 eq comparison should succeed");
        assert!(equal, "0.1_f32 should equal TOML float literal 0.1");
    }

    #[test]
    fn compare_eq_f32_decimal_literal_detects_real_mismatch() {
        let actual = SignalValue::F32(0.1_f32);
        let expected = toml::Value::Float(0.2_f64);
        let equal = compare_eq(&actual, &expected).expect("f32 eq comparison should succeed");
        assert!(!equal, "0.1_f32 should not equal TOML float literal 0.2");
    }
}
