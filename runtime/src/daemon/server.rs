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
use crate::ipc::{self, BoxedLocalStream};
use crate::protocol::{Request, RequestAction, Response};
use crate::shared::SharedRegion;
use crate::sim::error::SimError;
use crate::sim::project::Project;
use crate::sim::time::TimeEngine;
use crate::sim::types::{SignalType, SignalValue, SimCanBusDesc, SimCanFrame, SimSharedDesc};
use crate::trace::CsvTraceWriter;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split};
use tokio::sync::{mpsc, oneshot, watch};

pub struct DaemonState {
    session: String,
    socket_path: PathBuf,
    env: Option<String>,
    project: Project,
    can_attached: HashMap<String, AttachedCanBus>,
    dbc_overlays: HashMap<String, DbcBusOverlay>,
    shared_attached: HashMap<String, AttachedSharedChannel>,
    frame_state: HashMap<String, HashMap<u32, SimCanFrame>>,
    trace: DaemonTraceState,
    time: TimeEngine,
    realtime_tick_backlog: u64,
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

struct DaemonTraceState {
    active: Option<ActiveDaemonTrace>,
    last_path: Option<PathBuf>,
    last_row_count: u64,
    last_signal_count: usize,
    last_period_us: Option<u64>,
}

struct ActiveDaemonTrace {
    writer: CsvTraceWriter,
    signal_ids: Vec<u32>,
    period_ticks: u64,
    period_us: u64,
    next_due_tick: u64,
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
            dbc_overlays: HashMap::new(),
            shared_attached: HashMap::new(),
            frame_state: HashMap::new(),
            trace: DaemonTraceState {
                active: None,
                last_path: None,
                last_row_count: 0,
                last_signal_count: 0,
                last_period_us: None,
            },
            time: TimeEngine::default(),
            realtime_tick_backlog: 0,
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
}

pub async fn run_listener(
    session: String,
    socket_path: PathBuf,
    project: Project,
    env: Option<String>,
) -> Result<(), std::io::Error> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut listener = ipc::bind_listener(&socket_path).await?;
    ipc::create_endpoint_marker(&socket_path)?;
    let pid_path = crate::daemon::lifecycle::pid_path(&session);
    std::fs::write(&pid_path, std::process::id().to_string())?;
    crate::daemon::lifecycle::write_env_tag(&session, env.as_deref())
        .map_err(std::io::Error::other)?;

    let state = DaemonState::new(session.clone(), socket_path.clone(), project, env);
    let (action_tx, action_rx) = mpsc::channel::<ActionMessage>(256);
    let (worker_action_tx, worker_action_rx) = mpsc::channel::<ActionMessage>(256);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let tick_task = tokio::spawn(tick_ops::run_tick_task(
        state,
        action_rx,
        worker_action_rx,
        shutdown_tx,
    ));
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
                    Ok(stream) => {
                        let action_tx = action_tx.clone();
                        let worker_action_tx = worker_action_tx.clone();
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, action_tx, worker_action_tx).await;
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
    drop(worker_action_tx);
    let _ = tick_task.await;

    ipc::cleanup_endpoint(&socket_path);
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
    stream: BoxedLocalStream,
    action_tx: mpsc::Sender<ActionMessage>,
    worker_action_tx: mpsc::Sender<ActionMessage>,
) -> Result<(), std::io::Error> {
    let (read_half, mut write_half) = split(stream);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Ok(());
        }
        let response = match serde_json::from_str::<Request>(line.trim_end()) {
            Ok(request) => {
                let request_id = request.id;
                let (response_tx, response_rx) = oneshot::channel();
                let target_tx = if matches!(&request.action, RequestAction::Worker(_)) {
                    &worker_action_tx
                } else {
                    &action_tx
                };
                if target_tx
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
        let mut payload = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!("{{\"success\":false,\"error\":\"response serialization failed: {e}\"}}")
        });
        payload.push('\n');
        write_half.write_all(payload.as_bytes()).await?;
    }
}

async fn process_action_message(message: ActionMessage, state: &mut DaemonState) {
    let response = handle_action(message.request, state).await;
    let _ = message.response_tx.send(response);
}

async fn handle_action(request: Request, state: &mut DaemonState) -> Response {
    let id = request.id;
    let result = match request.action {
        RequestAction::Instance(action) => {
            action_router::dispatch_instance_action(action, state).await
        }
        RequestAction::Worker(action) => action_router::dispatch_worker_action(action, state).await,
        RequestAction::Env(_) => Err("env-owned action sent to instance daemon".to_string()),
    };

    match result {
        Ok(data) => Response::ok(id, data),
        Err(e) => Response::err(id, e),
    }
}
