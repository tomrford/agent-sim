use crate::config::load_config;
use crate::config::recipe::{ForSpec, PrintSpec, RecipeStep, toml_value_to_cli_string};
use crate::protocol::{
    Action, Request, Response, ResponseData, SessionInfoData, SignalData, SignalValueData,
    WatchSampleData, parse_duration_us,
};
use crate::sim::error::SimError;
use crate::sim::project::Project;
use crate::sim::time::TimeEngine;
use crate::sim::types::{SignalType, SignalValue};
use globset::{Glob, GlobMatcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::{Duration, sleep, timeout};

pub struct DaemonState {
    session: String,
    socket_path: PathBuf,
    project: Project,
    time: TimeEngine,
    shutdown: bool,
}

impl DaemonState {
    pub fn new(session: String, socket_path: PathBuf, project: Project) -> Self {
        Self {
            session,
            socket_path,
            project,
            time: TimeEngine::default(),
            shutdown: false,
        }
    }

    fn parse_value(signal_type: SignalType, raw: &str) -> Result<SignalValue, SimError> {
        match signal_type {
            SignalType::Bool => match raw {
                "true" | "1" | "True" | "TRUE" => Ok(SignalValue::Bool(true)),
                "false" | "0" | "False" | "FALSE" => Ok(SignalValue::Bool(false)),
                _ => Err(SimError::InvalidArg(format!("invalid bool value '{raw}'"))),
            },
            SignalType::U32 => raw
                .parse::<u32>()
                .map(SignalValue::U32)
                .map_err(|_| SimError::InvalidArg(format!("invalid u32 value '{raw}'"))),
            SignalType::I32 => raw
                .parse::<i32>()
                .map(SignalValue::I32)
                .map_err(|_| SimError::InvalidArg(format!("invalid i32 value '{raw}'"))),
            SignalType::F32 => raw
                .parse::<f32>()
                .map(SignalValue::F32)
                .map_err(|_| SimError::InvalidArg(format!("invalid f32 value '{raw}'"))),
            SignalType::F64 => raw
                .parse::<f64>()
                .map(SignalValue::F64)
                .map_err(|_| SimError::InvalidArg(format!("invalid f64 value '{raw}'"))),
        }
    }

    fn select_signal_ids(
        project: &Project,
        selectors: &[String],
    ) -> Result<Vec<u32>, Box<dyn std::error::Error + Send + Sync>> {
        if selectors.is_empty() {
            return Err("missing signal selectors".into());
        }
        let mut ids = BTreeSet::new();
        for selector in selectors {
            if selector == "*" {
                ids.extend(project.signals().iter().map(|s| s.id));
                continue;
            }
            if let Some(raw_id) = selector.strip_prefix('#') {
                let id = raw_id.parse::<u32>()?;
                if project.signal_by_id(id).is_none() {
                    return Err(format!("signal not found: '#{id}'").into());
                }
                ids.insert(id);
                continue;
            }
            if selector.contains('*') || selector.contains('?') || selector.contains('[') {
                let matcher = compile_glob(selector)?;
                let mut matched = false;
                for signal in project.signals() {
                    if matcher.is_match(&signal.name) {
                        ids.insert(signal.id);
                        matched = true;
                    }
                }
                if !matched {
                    return Err(format!("signal glob matched nothing: '{selector}'").into());
                }
                continue;
            }

            if let Some(id) = project.signal_id_by_name(selector) {
                ids.insert(id);
            } else {
                return Err(format!("signal not found: '{selector}'").into());
            }
        }
        Ok(ids.into_iter().collect())
    }
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, Box<dyn std::error::Error + Send + Sync>> {
    Ok(Glob::new(pattern)?.compile_matcher())
}

pub async fn run_listener(
    session: String,
    socket_path: PathBuf,
    project: Project,
) -> Result<(), std::io::Error> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    let pid_path = crate::daemon::lifecycle::pid_path(&session);
    std::fs::write(&pid_path, std::process::id().to_string())?;

    let mut state = DaemonState::new(session, socket_path.clone(), project);
    loop {
        let _ = state.time.tick_realtime(&state.project);

        let accepted = timeout(Duration::from_millis(20), listener.accept()).await;
        match accepted {
            Ok(Ok((stream, _addr))) => {
                handle_connection(stream, &mut state).await?;
                if state.shutdown {
                    break;
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {}
        }
    }

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if pid_path.exists() {
        let _ = std::fs::remove_file(pid_path);
    }
    Ok(())
}

async fn handle_connection(
    mut stream: UnixStream,
    state: &mut DaemonState,
) -> Result<(), std::io::Error> {
    let mut line = String::new();
    let mut reader = BufReader::new(&mut stream);
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => handle_action(request, state).await,
        Err(e) => Response {
            id: uuid::Uuid::new_v4(),
            success: false,
            data: None,
            error: Some(format!("invalid request json: {e}")),
        },
    };
    drop(reader);
    let mut payload = serde_json::to_string(&response).unwrap_or_else(|e| {
        format!("{{\"success\":false,\"error\":\"response serialization failed: {e}\"}}")
    });
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    Ok(())
}

async fn handle_action(request: Request, state: &mut DaemonState) -> Response {
    let id = request.id;
    let result = dispatch_action(request.action, state).await;

    match result {
        Ok(data) => Response::ok(id, data),
        Err(e) => Response::err(id, e),
    }
}

#[async_recursion::async_recursion]
async fn dispatch_action(action: Action, state: &mut DaemonState) -> Result<ResponseData, String> {
    match action {
        Action::Ping => Ok(ResponseData::Ack),
        Action::Load { libpath } => {
            let bound = state.project.libpath.display().to_string();
            if libpath != bound {
                return Err(format!(
                    "daemon already bound to '{bound}'; start a new session for a different DLL"
                ));
            }
            Ok(ResponseData::Loaded {
                libpath: bound,
                signal_count: state.project.signals().len(),
            })
        }
        Action::Info => Ok(ResponseData::ProjectInfo {
            libpath: state.project.libpath.display().to_string(),
            tick_duration_us: state.project.tick_duration_us(),
            signal_count: state.project.signals().len(),
        }),
        Action::Signals => {
            let signals = state
                .project
                .signals()
                .iter()
                .map(|s| SignalData {
                    id: s.id,
                    name: s.name.clone(),
                    signal_type: s.signal_type,
                    units: s.units.clone(),
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::Signals { signals })
        }
        Action::Reset => {
            state.project.reset().map_err(|e| e.to_string())?;
            Ok(ResponseData::Ack)
        }
        Action::Get { selectors } => {
            let ids = DaemonState::select_signal_ids(&state.project, &selectors)
                .map_err(|e| SimError::InvalidSignal(e.to_string()).to_string())?;
            let mut values = Vec::new();
            for id in ids {
                let signal = state
                    .project
                    .signal_by_id(id)
                    .ok_or_else(|| SimError::InvalidSignal(format!("#{id}")).to_string())?;
                let value = state.project.read(signal).map_err(|e| e.to_string())?;
                values.push(SignalValueData {
                    id: signal.id,
                    name: signal.name.clone(),
                    signal_type: signal.signal_type,
                    value,
                    units: signal.units.clone(),
                });
            }
            Ok(ResponseData::SignalValues { values })
        }
        Action::Set { writes } => {
            let mut applied = 0_usize;
            for (selector, raw_value) in writes {
                let ids = DaemonState::select_signal_ids(&state.project, std::slice::from_ref(&selector))
                    .map_err(|e| SimError::InvalidSignal(e.to_string()).to_string())?;
                for id in ids {
                    let signal = state
                        .project
                        .signal_by_id(id)
                        .ok_or_else(|| SimError::InvalidSignal(format!("#{id}")).to_string())?;
                    let value = DaemonState::parse_value(signal.signal_type, &raw_value)
                        .map_err(|e| e.to_string())?;
                    state
                        .project
                        .write(signal, &value)
                        .map_err(|e| e.to_string())?;
                    applied += 1;
                }
            }
            Ok(ResponseData::SetResult {
                writes_applied: applied,
            })
        }
        Action::TimeStart => {
            state.time.start().map_err(|e| e.to_string())?;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        Action::TimePause => {
            state.time.pause().map_err(|e| e.to_string())?;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        Action::TimeStep { duration } => {
            let duration_us = parse_duration_us(&duration).map_err(|e| e.to_string())?;
            let step = state
                .time
                .step(&state.project, duration_us)
                .map_err(|e| e.to_string())?;
            Ok(ResponseData::TimeAdvanced {
                requested_us: step.requested_us,
                advanced_ticks: step.advanced_ticks,
                advanced_us: step.advanced_us,
            })
        }
        Action::TimeSpeed { multiplier } => {
            if let Some(multiplier) = multiplier {
                state
                    .time
                    .set_speed(multiplier)
                    .map_err(|e| e.to_string())?;
            }
            Ok(ResponseData::Speed {
                speed: state.time.speed(),
            })
        }
        Action::TimeStatus => {
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        Action::Watch {
            selector,
            interval_ms,
            samples,
        } => {
            let ids = DaemonState::select_signal_ids(&state.project, std::slice::from_ref(&selector))
                .map_err(|e| SimError::InvalidSignal(e.to_string()).to_string())?;
            let id = *ids
                .first()
                .ok_or_else(|| SimError::InvalidSignal(selector.clone()).to_string())?;
            let signal = state
                .project
                .signal_by_id(id)
                .ok_or_else(|| SimError::InvalidSignal(selector.clone()).to_string())?
                .clone();
            let count = samples.unwrap_or(10).max(1);
            let mut out = Vec::new();
            for _ in 0..count {
                let value = state.project.read(&signal).map_err(|e| e.to_string())?;
                let status = state.time.status(state.project.tick_duration_us());
                out.push(WatchSampleData {
                    tick: status.elapsed_ticks,
                    time_us: status.elapsed_time_us,
                    signal: signal.name.clone(),
                    value,
                });
                sleep(Duration::from_millis(interval_ms.max(1))).await;
            }
            Ok(ResponseData::WatchSamples { samples: out })
        }
        Action::RunRecipe {
            recipe,
            dry_run,
            config,
        } => {
            let mut events = Vec::new();
            let config =
                load_config(config.as_deref().map(Path::new)).map_err(|e| e.to_string())?;
            let recipe_def = config.recipe(&recipe).map_err(|e| e.to_string())?;
            for step in &recipe_def.steps {
                execute_recipe_step(step, dry_run, state, &mut events)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(ResponseData::RecipeResult {
                recipe,
                dry_run,
                steps_executed: recipe_def.steps.len(),
                events,
            })
        }
        Action::SessionStatus => Ok(ResponseData::SessionStatus {
            session: state.session.clone(),
            socket_path: state.socket_path.display().to_string(),
            running: true,
        }),
        Action::SessionList => {
            let sessions = crate::daemon::lifecycle::list_sessions()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(name, socket_path, running)| SessionInfoData {
                    name,
                    socket_path: socket_path.display().to_string(),
                    running,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::SessionList { sessions })
        }
        Action::Close => {
            state.shutdown = true;
            Ok(ResponseData::Ack)
        }
    }
}

#[async_recursion::async_recursion]
async fn execute_recipe_step(
    step: &RecipeStep,
    dry_run: bool,
    state: &mut DaemonState,
    events: &mut Vec<String>,
) -> Result<(), String> {
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
            if !dry_run {
                let req = Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::Set { writes },
                };
                dispatch_action(req.action, state).await?;
            }
        }
        RecipeStep::Step { step } => {
            events.push(format!("step {step}"));
            if !dry_run {
                let req = Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::TimeStep {
                        duration: step.clone(),
                    },
                };
                dispatch_action(req.action, state).await?;
            }
        }
        RecipeStep::Print { print } => {
            let selectors = match print {
                PrintSpec::All(value) if value == "*" => vec!["*".to_string()],
                PrintSpec::All(value) => vec![value.clone()],
                PrintSpec::Signals(values) => values.clone(),
            };
            events.push(format!("print {}", selectors.join(",")));
            if !dry_run {
                let req = Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::Get { selectors },
                };
                dispatch_action(req.action, state).await?;
            }
        }
        RecipeStep::Speed { speed } => {
            events.push(format!("speed {speed}"));
            if !dry_run {
                let req = Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::TimeSpeed {
                        multiplier: Some(*speed),
                    },
                };
                dispatch_action(req.action, state).await?;
            }
        }
        RecipeStep::Reset { .. } => {
            events.push("reset".to_string());
            if !dry_run {
                let req = Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::Reset,
                };
                dispatch_action(req.action, state).await?;
            }
        }
        RecipeStep::Sleep { sleep: ms } => {
            events.push(format!("sleep {ms}ms"));
            if !dry_run {
                sleep(Duration::from_millis(*ms)).await;
            }
        }
        RecipeStep::For { r#for } => execute_for_step(r#for, dry_run, state, events).await?,
    }
    Ok(())
}

#[async_recursion::async_recursion]
async fn execute_for_step(
    spec: &ForSpec,
    dry_run: bool,
    state: &mut DaemonState,
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
        if !dry_run {
            let mut writes = BTreeMap::new();
            writes.insert(spec.signal.clone(), current.to_string());
            let req = Request {
                id: uuid::Uuid::new_v4(),
                action: Action::Set { writes },
            };
            dispatch_action(req.action, state).await?;
        }
        for step in &spec.each {
            execute_recipe_step(step, dry_run, state, events).await?;
        }
        current += spec.by;
    }
    Ok(())
}
