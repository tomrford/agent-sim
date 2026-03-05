#[path = "server/action_router.rs"]
mod action_router;
#[path = "server/can_ops.rs"]
mod can_ops;
#[path = "server/shared_ops.rs"]
mod shared_ops;
#[path = "server/tick_ops.rs"]
mod tick_ops;

use crate::can::CanSocket;
use crate::can::dbc::DbcBusOverlay;
use crate::protocol::{Request, Response};
use crate::shared::SharedRegion;
use crate::sim::error::SimError;
use crate::sim::project::Project;
use crate::sim::time::TimeEngine;
use crate::sim::types::{SignalType, SignalValue, SimCanBusDesc, SimCanFrame, SimSharedDesc};
use globset::{Glob, GlobMatcher};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch};

pub struct DaemonState {
    session: String,
    socket_path: PathBuf,
    env: Option<String>,
    project: Project,
    can_attached: HashMap<String, AttachedCanBus>,
    shared_attached: HashMap<String, AttachedSharedChannel>,
    dbc_overlays: HashMap<String, DbcBusOverlay>,
    frame_state: HashMap<String, HashMap<u32, SimCanFrame>>,
    time: TimeEngine,
    shutdown: bool,
}

struct AttachedCanBus {
    meta: SimCanBusDesc,
    socket: CanSocket,
}

struct AttachedSharedChannel {
    meta: SimSharedDesc,
    region: SharedRegion,
    writer: bool,
}

struct ActionMessage {
    request: Request,
    response_tx: oneshot::Sender<Response>,
}

impl DaemonState {
    pub fn new(
        session: String,
        socket_path: PathBuf,
        project: Project,
        env: Option<String>,
    ) -> Self {
        Self {
            session,
            socket_path,
            env,
            project,
            can_attached: HashMap::new(),
            shared_attached: HashMap::new(),
            dbc_overlays: HashMap::new(),
            frame_state: HashMap::new(),
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
    env: Option<String>,
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
    crate::daemon::lifecycle::write_env_tag(&session, env.as_deref())
        .map_err(std::io::Error::other)?;

    let state = DaemonState::new(session.clone(), socket_path.clone(), project, env);
    let (action_tx, action_rx) = mpsc::channel::<ActionMessage>(256);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let tick_task = tokio::spawn(tick_ops::run_tick_task(state, action_rx, shutdown_tx));
    let mut listener_error = None;

    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                match changed {
                    Ok(()) if *shutdown_rx.borrow() => break,
                    Ok(()) => {}
                    Err(_) => break,
                }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let action_tx = action_tx.clone();
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, action_tx).await;
                        });
                    }
                    Err(e) => {
                        listener_error = Some(e);
                        break;
                    }
                }
            }
        }
    }

    drop(action_tx);
    let _ = tick_task.await;

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if pid_path.exists() {
        let _ = std::fs::remove_file(pid_path);
    }
    crate::daemon::lifecycle::remove_env_tag(&session);

    if let Some(err) = listener_error {
        return Err(err);
    }
    Ok(())
}

async fn handle_connection(
    mut stream: UnixStream,
    action_tx: mpsc::Sender<ActionMessage>,
) -> Result<(), std::io::Error> {
    let mut line = String::new();
    let mut reader = BufReader::new(&mut stream);
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => {
            let request_id = request.id;
            let (response_tx, response_rx) = oneshot::channel();
            if action_tx
                .send(ActionMessage {
                    request,
                    response_tx,
                })
                .await
                .is_err()
            {
                Response::err(request_id, "daemon unavailable")
            } else {
                match response_rx.await {
                    Ok(response) => response,
                    Err(_) => Response::err(request_id, "daemon unavailable"),
                }
            }
        }
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

async fn process_action_message(message: ActionMessage, state: &mut DaemonState) {
    let response = handle_action(message.request, state).await;
    let _ = message.response_tx.send(response);
}

async fn handle_action(request: Request, state: &mut DaemonState) -> Response {
    let id = request.id;
    let result = action_router::dispatch_action(request.action, state).await;

    match result {
        Ok(data) => Response::ok(id, data),
        Err(e) => Response::err(id, e),
    }
}
