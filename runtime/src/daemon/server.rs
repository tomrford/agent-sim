use crate::can::CanSocket;
use crate::can::dbc::{DbcBusOverlay, decode_signal, encode_signal, frame_key_from_frame};
use crate::protocol::{
    Action, CanBusData, Request, Response, ResponseData, SessionInfoData, SharedChannelData,
    SharedSlotValueData, SignalData, SignalValueData, parse_duration_us,
};
use crate::shared::SharedRegion;
use crate::sim::error::SimError;
use crate::sim::project::Project;
use crate::sim::time::TimeEngine;
use crate::sim::types::{
    CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RESERVED_MASK,
    CAN_FLAG_RTR, SignalType, SignalValue, SimCanBusDesc, SimCanFrame, SimSharedDesc,
};
use globset::{Glob, GlobMatcher};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Duration, timeout};

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

    let tick_task = tokio::spawn(run_tick_task(state, action_rx, shutdown_tx));
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

async fn run_tick_task(
    mut state: DaemonState,
    mut action_rx: mpsc::Receiver<ActionMessage>,
    shutdown_tx: watch::Sender<bool>,
) {
    loop {
        while let Ok(message) = action_rx.try_recv() {
            process_action_message(message, &mut state).await;
        }

        if state.shutdown {
            break;
        }

        let due_ticks = state
            .time
            .tick_realtime_due(state.project.tick_duration_us());
        let _ = advance_project_ticks(&mut state, due_ticks);

        if state.shutdown {
            break;
        }

        let sleep_duration = if state.time.is_running() {
            Duration::from_millis(1)
        } else {
            Duration::from_millis(5)
        };
        match timeout(sleep_duration, action_rx.recv()).await {
            Ok(Some(message)) => process_action_message(message, &mut state).await,
            Ok(None) => break,
            Err(_) => {}
        }
    }

    let _ = shutdown_tx.send(true);
}

async fn process_action_message(message: ActionMessage, state: &mut DaemonState) {
    let response = handle_action(message.request, state).await;
    let _ = message.response_tx.send(response);
}

async fn handle_action(request: Request, state: &mut DaemonState) -> Response {
    let id = request.id;
    let result = dispatch_action(request.action, state).await;

    match result {
        Ok(data) => Response::ok(id, data),
        Err(e) => Response::err(id, e),
    }
}

async fn dispatch_action(action: Action, state: &mut DaemonState) -> Result<ResponseData, String> {
    match action {
        Action::Ping => Ok(ResponseData::Ack),
        Action::Load { libpath, .. } => {
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
            state.time.reset();
            Ok(ResponseData::Ack)
        }
        Action::Get { selectors } => {
            let mut values = Vec::new();
            let mut native_selectors = Vec::new();

            for selector in selectors {
                if selector.starts_with("can.") {
                    values.extend(get_can_signal_values(state, &selector)?);
                } else {
                    native_selectors.push(selector);
                }
            }

            if !native_selectors.is_empty() {
                let ids = DaemonState::select_signal_ids(&state.project, &native_selectors)
                    .map_err(|e| SimError::InvalidSignal(e.to_string()).to_string())?;
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
            }
            Ok(ResponseData::SignalValues { values })
        }
        Action::Set { writes } => {
            let mut applied = 0_usize;
            for (selector, raw_value) in writes {
                if selector.starts_with("can.") {
                    write_can_signal(state, &selector, &raw_value)?;
                    applied += 1;
                    continue;
                }

                let ids =
                    DaemonState::select_signal_ids(&state.project, std::slice::from_ref(&selector))
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
                .step_ticks(state.project.tick_duration_us(), duration_us)
                .map_err(|e| e.to_string())?;
            advance_project_ticks(state, step.advanced_ticks).map_err(|e| e.to_string())?;
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
        Action::CanBuses => {
            let buses = state
                .project
                .can_buses()
                .iter()
                .map(|bus| CanBusData {
                    id: bus.id,
                    name: bus.name.clone(),
                    bitrate: bus.bitrate,
                    bitrate_data: bus.bitrate_data,
                    fd_capable: bus.fd_capable,
                    attached_iface: state
                        .can_attached
                        .get(&bus.name)
                        .map(|attached| attached.socket.iface().to_string()),
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::CanBuses { buses })
        }
        Action::CanAttach {
            bus_name,
            vcan_iface,
        } => {
            if state.can_attached.contains_key(&bus_name) {
                return Err(format!("CAN bus '{bus_name}' is already attached"));
            }
            let meta = find_can_bus_meta(state, &bus_name)?;
            let socket = CanSocket::open(&vcan_iface, meta.fd_capable)?;
            state
                .can_attached
                .insert(bus_name.clone(), AttachedCanBus { meta, socket });
            Ok(ResponseData::Ack)
        }
        Action::CanDetach { bus_name } => {
            if state.can_attached.remove(&bus_name).is_none() {
                return Err(format!("CAN bus '{bus_name}' is not attached"));
            }
            Ok(ResponseData::Ack)
        }
        Action::CanLoadDbc { bus_name, path } => {
            let _ = find_can_bus_meta(state, &bus_name)?;
            let overlay = DbcBusOverlay::load(std::path::Path::new(&path))?;
            let signal_count = overlay.signal_names().count();
            state.dbc_overlays.insert(bus_name.clone(), overlay);
            Ok(ResponseData::DbcLoaded {
                bus: bus_name,
                signal_count,
            })
        }
        Action::SharedList => {
            let channels = state
                .project
                .shared_channels()
                .iter()
                .map(|channel| SharedChannelData {
                    id: channel.id,
                    name: channel.name.clone(),
                    slot_count: channel.slot_count,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::SharedChannels { channels })
        }
        Action::SharedAttach {
            channel_name,
            path,
            writer,
            writer_session,
        } => {
            if state.shared_attached.contains_key(&channel_name) {
                return Err(format!(
                    "shared channel '{channel_name}' is already attached"
                ));
            }
            let meta = find_shared_channel_meta(state, &channel_name)?;
            let region = SharedRegion::open(
                std::path::Path::new(&path),
                meta.slot_count as usize,
                &writer_session,
                writer,
            )?;
            state.shared_attached.insert(
                channel_name.clone(),
                AttachedSharedChannel {
                    meta,
                    region,
                    writer,
                },
            );
            Ok(ResponseData::Ack)
        }
        Action::SharedGet { channel_name } => {
            let attachment = state
                .shared_attached
                .get(&channel_name)
                .ok_or_else(|| format!("shared channel '{channel_name}' is not attached"))?;
            let slots = attachment
                .region
                .read_snapshot()
                .into_iter()
                .map(|slot| SharedSlotValueData {
                    slot_id: slot.slot_id,
                    signal_type: slot.value.signal_type(),
                    value: slot.value,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::SharedValues {
                channel: channel_name,
                slots,
            })
        }
        Action::CanSend {
            bus_name,
            arb_id,
            data_hex,
            flags,
        } => {
            let attachment = state
                .can_attached
                .get(&bus_name)
                .ok_or_else(|| format!("CAN bus '{bus_name}' is not attached"))?;
            let payload = parse_data_hex(&data_hex)?;
            let mut data = [0_u8; 64];
            data[..payload.len()].copy_from_slice(&payload);
            let frame = SimCanFrame {
                arb_id,
                len: payload.len() as u8,
                flags: flags.unwrap_or(0),
                data,
            };
            validate_can_frame(&attachment.meta, &frame)?;
            attachment.socket.send(&frame)?;
            record_frame(state, &bus_name, &frame);
            Ok(ResponseData::CanSend {
                bus: bus_name,
                arb_id,
                len: frame.len,
            })
        }
        Action::SessionStatus => Ok(ResponseData::SessionStatus {
            session: state.session.clone(),
            socket_path: state.socket_path.display().to_string(),
            running: true,
            env: state.env.clone(),
        }),
        Action::SessionList => {
            let sessions = crate::daemon::lifecycle::list_sessions()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(name, socket_path, running, env)| SessionInfoData {
                    name,
                    socket_path: socket_path.display().to_string(),
                    running,
                    env,
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

fn advance_project_ticks(state: &mut DaemonState, ticks: u64) -> Result<(), String> {
    let mut processed = 0_u64;
    for _ in 0..ticks {
        if let Err(err) = process_can_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = process_shared_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = state.project.tick() {
            state.time.advance_ticks(processed);
            return Err(err.to_string());
        }
        if let Err(err) = process_can_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        if let Err(err) = process_shared_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        processed = processed.saturating_add(1);
    }
    state.time.advance_ticks(processed);
    Ok(())
}

fn process_can_rx(state: &mut DaemonState) -> Result<(), String> {
    let mut frame_updates = Vec::new();
    for (bus_name, attachment) in &mut state.can_attached {
        let frames = attachment.socket.recv_all()?;
        if frames.is_empty() {
            continue;
        }
        for frame in &frames {
            validate_can_frame(&attachment.meta, frame)?;
            frame_updates.push((bus_name.clone(), frame.clone()));
        }
        state
            .project
            .can_rx(attachment.meta.id, &frames)
            .map_err(|e| format!("sim_can_rx failed for bus '{bus_name}': {e}"))?;
    }
    for (bus_name, frame) in frame_updates {
        record_frame(state, &bus_name, &frame);
    }
    Ok(())
}

fn process_can_tx(state: &mut DaemonState) -> Result<(), String> {
    let mut frame_updates = Vec::new();
    for (bus_name, attachment) in &mut state.can_attached {
        let tx_frames = state
            .project
            .can_tx(attachment.meta.id)
            .map_err(|e| format!("sim_can_tx failed for bus '{bus_name}': {e}"))?;
        for frame in tx_frames {
            validate_can_frame(&attachment.meta, &frame)?;
            attachment.socket.send(&frame)?;
            frame_updates.push((bus_name.clone(), frame));
        }
    }
    for (bus_name, frame) in frame_updates {
        record_frame(state, &bus_name, &frame);
    }
    Ok(())
}

fn process_shared_rx(state: &mut DaemonState) -> Result<(), String> {
    for (channel_name, attachment) in &state.shared_attached {
        let slots = attachment.region.read_snapshot();
        if slots.is_empty() {
            continue;
        }
        state
            .project
            .shared_read(attachment.meta.id, &slots)
            .map_err(|e| format!("sim_shared_read failed for channel '{channel_name}': {e}"))?;
    }
    Ok(())
}

fn process_shared_tx(state: &mut DaemonState) -> Result<(), String> {
    for (channel_name, attachment) in &mut state.shared_attached {
        if !attachment.writer {
            continue;
        }
        let slots = state
            .project
            .shared_write(attachment.meta.id)
            .map_err(|e| format!("sim_shared_write failed for channel '{channel_name}': {e}"))?;
        if slots.is_empty() {
            continue;
        }
        attachment
            .region
            .publish(&slots)
            .map_err(|e| format!("failed publishing shared channel '{channel_name}': {e}"))?;
    }
    Ok(())
}

fn find_can_bus_meta(state: &DaemonState, bus_name: &str) -> Result<SimCanBusDesc, String> {
    state
        .project
        .can_buses()
        .iter()
        .find(|bus| bus.name == bus_name)
        .cloned()
        .ok_or_else(|| format!("CAN bus '{bus_name}' not declared by loaded project"))
}

fn find_shared_channel_meta(
    state: &DaemonState,
    channel_name: &str,
) -> Result<SimSharedDesc, String> {
    state
        .project
        .shared_channels()
        .iter()
        .find(|channel| channel.name == channel_name)
        .cloned()
        .ok_or_else(|| format!("shared channel '{channel_name}' not declared by loaded project"))
}

fn get_can_signal_values(
    state: &DaemonState,
    selector: &str,
) -> Result<Vec<SignalValueData>, String> {
    let (bus_name, signal_selector) = parse_can_selector(selector)?;
    let overlay = state
        .dbc_overlays
        .get(bus_name)
        .ok_or_else(|| format!("no DBC loaded for CAN bus '{bus_name}'"))?;
    if signal_selector == "*" {
        let mut values = Vec::new();
        let mut names = overlay.signal_names().cloned().collect::<Vec<_>>();
        names.sort();
        for name in names {
            let signal = overlay
                .signal(&name)
                .ok_or_else(|| format!("DBC signal '{name}' not found"))?;
            let frame = latest_frame_for_signal(state, bus_name, signal)?;
            let value = decode_signal(frame, signal)?;
            values.push(SignalValueData {
                id: signal.arb_id,
                name: format!("can.{bus_name}.{}", signal.name),
                signal_type: SignalType::F64,
                value: SignalValue::F64(value),
                units: signal.unit.clone(),
            });
        }
        return Ok(values);
    }

    let signal = overlay
        .signal(signal_selector)
        .ok_or_else(|| format!("CAN signal '{signal_selector}' not found on bus '{bus_name}'"))?;
    let frame = latest_frame_for_signal(state, bus_name, signal)?;
    let value = decode_signal(frame, signal)?;
    Ok(vec![SignalValueData {
        id: signal.arb_id,
        name: format!("can.{bus_name}.{}", signal.name),
        signal_type: SignalType::F64,
        value: SignalValue::F64(value),
        units: signal.unit.clone(),
    }])
}

fn write_can_signal(
    state: &mut DaemonState,
    selector: &str,
    raw_value: &str,
) -> Result<(), String> {
    let (bus_name, signal_name) = parse_can_selector(selector)?;
    if signal_name == "*" {
        return Err(format!(
            "wildcard writes are not supported for CAN selectors: '{selector}'"
        ));
    }
    let physical_value = raw_value
        .parse::<f64>()
        .map_err(|_| format!("invalid CAN signal value '{raw_value}'"))?;

    let signal = {
        let overlay = state
            .dbc_overlays
            .get(bus_name)
            .ok_or_else(|| format!("no DBC loaded for CAN bus '{bus_name}'"))?;
        overlay
            .signal(signal_name)
            .cloned()
            .ok_or_else(|| format!("CAN signal '{signal_name}' not found on bus '{bus_name}'"))?
    };

    let mut frame = state
        .frame_state
        .get(bus_name)
        .and_then(|frames| frames.get(&signal.frame_key))
        .cloned()
        .unwrap_or_else(|| {
            let data = [0_u8; 64];
            let len = signal.message_size.min(64);
            SimCanFrame {
                arb_id: signal.arb_id,
                len,
                flags: if signal.extended {
                    CAN_FLAG_EXTENDED
                } else {
                    0
                },
                data,
            }
        });
    frame.arb_id = signal.arb_id;
    if signal.extended {
        frame.flags |= CAN_FLAG_EXTENDED;
    } else {
        frame.flags &= !CAN_FLAG_EXTENDED;
    }

    encode_signal(&mut frame, &signal, physical_value)?;

    let attachment = state
        .can_attached
        .get(bus_name)
        .ok_or_else(|| format!("CAN bus '{bus_name}' is not attached"))?;
    validate_can_frame(&attachment.meta, &frame)?;
    attachment.socket.send(&frame)?;
    record_frame(state, bus_name, &frame);
    Ok(())
}

fn latest_frame_for_signal<'a>(
    state: &'a DaemonState,
    bus_name: &str,
    signal: &crate::can::dbc::DbcSignalDef,
) -> Result<&'a SimCanFrame, String> {
    state
        .frame_state
        .get(bus_name)
        .and_then(|frames| frames.get(&signal.frame_key))
        .ok_or_else(|| {
            format!(
                "no frame observed yet for CAN signal 'can.{bus_name}.{}' (arb_id=0x{:X})",
                signal.name, signal.arb_id
            )
        })
}

fn parse_can_selector(selector: &str) -> Result<(&str, &str), String> {
    let Some(rest) = selector.strip_prefix("can.") else {
        return Err(format!("invalid CAN selector '{selector}'"));
    };
    let Some((bus_name, signal_name)) = rest.split_once('.') else {
        return Err(format!(
            "invalid CAN selector '{selector}'; expected can.<bus>.<signal>"
        ));
    };
    if bus_name.is_empty() || signal_name.is_empty() {
        return Err(format!(
            "invalid CAN selector '{selector}'; expected can.<bus>.<signal>"
        ));
    }
    Ok((bus_name, signal_name))
}

fn record_frame(state: &mut DaemonState, bus_name: &str, frame: &SimCanFrame) {
    let bus_frames = state.frame_state.entry(bus_name.to_string()).or_default();
    bus_frames.insert(frame_key_from_frame(frame), frame.clone());
}

fn parse_data_hex(raw: &str) -> Result<Vec<u8>, String> {
    let compact = raw
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '_')
        .collect::<String>();
    if compact.len() % 2 != 0 {
        return Err(format!(
            "invalid CAN payload hex '{raw}': expected an even number of hex characters"
        ));
    }
    if compact.len() / 2 > 64 {
        return Err(format!(
            "invalid CAN payload hex '{raw}': payload exceeds 64 bytes"
        ));
    }
    let mut payload = Vec::with_capacity(compact.len() / 2);
    let bytes = compact.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() {
        let hi = bytes[idx] as char;
        let lo = bytes[idx + 1] as char;
        let pair = format!("{hi}{lo}");
        let value = u8::from_str_radix(&pair, 16)
            .map_err(|_| format!("invalid CAN payload hex '{raw}': bad byte '{pair}'"))?;
        payload.push(value);
        idx += 2;
    }
    Ok(payload)
}

fn validate_can_frame(bus: &SimCanBusDesc, frame: &SimCanFrame) -> Result<(), String> {
    if (frame.flags & CAN_FLAG_RESERVED_MASK) != 0 {
        return Err(format!(
            "CAN frame for bus '{}' has reserved flag bits set",
            bus.name
        ));
    }
    if (frame.flags & CAN_FLAG_EXTENDED) != 0 {
        if frame.arb_id > 0x1FFF_FFFF {
            return Err(format!(
                "CAN frame for bus '{}' has invalid extended arbitration id 0x{:X}",
                bus.name, frame.arb_id
            ));
        }
    } else if frame.arb_id > 0x7FF {
        return Err(format!(
            "CAN frame for bus '{}' has invalid standard arbitration id 0x{:X}",
            bus.name, frame.arb_id
        ));
    }
    if frame.len > 64 {
        return Err(format!(
            "CAN frame for bus '{}' has invalid payload length {}",
            bus.name, frame.len
        ));
    }

    let fd_requested =
        (frame.flags & CAN_FLAG_FD) != 0 || (frame.flags & (CAN_FLAG_BRS | CAN_FLAG_ESI)) != 0;
    if fd_requested {
        if !bus.fd_capable {
            return Err(format!(
                "CAN bus '{}' is classic-only and cannot carry FD frames",
                bus.name
            ));
        }
        if !is_valid_can_fd_length(frame.len) {
            return Err(format!(
                "CAN FD frame for bus '{}' has invalid length {}; valid lengths are 0-8,12,16,20,24,32,48,64",
                bus.name, frame.len
            ));
        }
        if (frame.flags & CAN_FLAG_RTR) != 0 {
            return Err(format!(
                "CAN FD frame for bus '{}' cannot set RTR flag",
                bus.name
            ));
        }
    } else if frame.len > 8 {
        return Err(format!(
            "classic CAN frame for bus '{}' has invalid length {}",
            bus.name, frame.len
        ));
    }

    Ok(())
}

fn is_valid_can_fd_length(len: u8) -> bool {
    matches!(len, 0..=8 | 12 | 16 | 20 | 24 | 32 | 48 | 64)
}
