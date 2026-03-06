use crate::can::CanSocket;
use crate::can::dbc::{DbcBusOverlay, frame_key_from_frame};
use crate::connection::send_request;
use crate::daemon::lifecycle::{kill_pid, read_pid};
use crate::envd::lifecycle::pid_path;
use crate::envd::spec::{EnvCanBusMemberSpec, EnvInstanceSpec, EnvSharedChannelSpec, EnvSpec};
use crate::load::write_load_spec;
use crate::protocol::{
    Action, CanBusData, CanBusFramesData, CanFrameData, CanScheduleData, Request, Response,
    ResponseData, parse_duration_us,
};
use crate::sim::time::TimeEngine;
use crate::sim::types::{
    CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RESERVED_MASK,
    CAN_FLAG_RTR, SimCanFrame,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio::time::{Duration, sleep, timeout};

struct EnvState {
    name: String,
    socket_path: PathBuf,
    tick_duration_us: u32,
    instances: Vec<String>,
    time: TimeEngine,
    can_buses: BTreeMap<String, EnvCanBusState>,
    instance_bus_map: HashMap<(String, String), String>,
    ignored_instance_buses: HashSet<(String, String)>,
    shutdown: bool,
}

struct EnvCanBusState {
    name: String,
    vcan_iface: String,
    fd_capable: bool,
    bitrate: u32,
    bitrate_data: u32,
    members: Vec<EnvCanBusMemberSpec>,
    socket: CanSocket,
    dbc: Option<DbcBusOverlay>,
    latest_frames: HashMap<u32, SimCanFrame>,
    pending_delivery: Vec<PendingFrame>,
    schedules: BTreeMap<String, CanScheduleJob>,
}

#[derive(Clone)]
struct PendingFrame {
    source_instance: Option<String>,
    frame: SimCanFrame,
}

#[derive(Clone)]
struct CanScheduleJob {
    job_id: String,
    arb_id: u32,
    flags: u8,
    data_hex: String,
    frame: SimCanFrame,
    every_ticks: u64,
    next_due_tick: u64,
    enabled: bool,
}

impl EnvState {
    async fn bootstrap(socket_path: PathBuf, env_spec: EnvSpec) -> Result<Self, String> {
        let mut started_instances = Vec::new();
        let result = Self::bootstrap_inner(socket_path, &env_spec, &mut started_instances).await;
        if result.is_err() {
            rollback_instances(&started_instances).await;
        }
        result
    }

    async fn bootstrap_inner(
        socket_path: PathBuf,
        env_spec: &EnvSpec,
        started_instances: &mut Vec<String>,
    ) -> Result<Self, String> {
        let mut tick_duration_us = None;
        let mut instance_can_buses: HashMap<String, Vec<CanBusData>> = HashMap::new();

        for instance in &env_spec.instances {
            bootstrap_instance_detached(instance).await?;
            started_instances.push(instance.name.clone());

            let info = send_request(
                &instance.name,
                &Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::Info,
                },
            )
            .await
            .map_err(|err| err.to_string())?;
            let ResponseData::ProjectInfo {
                tick_duration_us: instance_tick_us,
                ..
            } = info
                .data
                .ok_or_else(|| format!("missing info payload for instance '{}'", instance.name))?
            else {
                return Err(format!(
                    "unexpected info payload while bootstrapping instance '{}'",
                    instance.name
                ));
            };
            match tick_duration_us {
                Some(expected) if expected != instance_tick_us => {
                    return Err(format!(
                        "env '{}' requires matching tick durations, but instance '{}' reports {}us and another member reports {}us",
                        env_spec.name, instance.name, instance_tick_us, expected
                    ));
                }
                None => tick_duration_us = Some(instance_tick_us),
                _ => {}
            }

            let can_buses = send_request(
                &instance.name,
                &Request {
                    id: uuid::Uuid::new_v4(),
                    action: Action::WorkerCanBuses,
                },
            )
            .await
            .map_err(|err| err.to_string())?;
            let ResponseData::CanBuses { buses } = can_buses.data.ok_or_else(|| {
                format!(
                    "missing CAN bus payload while bootstrapping instance '{}'",
                    instance.name
                )
            })?
            else {
                return Err(format!(
                    "unexpected CAN bus payload while bootstrapping instance '{}'",
                    instance.name
                ));
            };
            instance_can_buses.insert(instance.name.clone(), buses);
        }

        let mut instance_bus_map = HashMap::new();
        let mut can_buses = BTreeMap::new();
        for bus_spec in &env_spec.can_buses {
            let mut fd_capable = false;
            let mut bitrate = 0_u32;
            let mut bitrate_data = 0_u32;
            for member in &bus_spec.members {
                let member_buses =
                    instance_can_buses
                        .get(&member.instance_name)
                        .ok_or_else(|| {
                            format!(
                                "missing CAN bus registry for instance '{}'",
                                member.instance_name
                            )
                        })?;
                let meta = member_buses
                    .iter()
                    .find(|bus| bus.name == member.bus_name)
                    .ok_or_else(|| {
                        format!(
                            "env CAN bus '{}' references missing bus '{}:{}'",
                            bus_spec.name, member.instance_name, member.bus_name
                        )
                    })?;
                fd_capable |= meta.fd_capable;
                bitrate = bitrate.max(meta.bitrate);
                bitrate_data = bitrate_data.max(meta.bitrate_data);
                if instance_bus_map
                    .insert(
                        (member.instance_name.clone(), member.bus_name.clone()),
                        bus_spec.name.clone(),
                    )
                    .is_some()
                {
                    return Err(format!(
                        "instance '{}:{}' is attached to multiple env CAN buses",
                        member.instance_name, member.bus_name
                    ));
                }
            }

            let socket = CanSocket::open(&bus_spec.vcan_iface, fd_capable)?;
            let dbc = match &bus_spec.dbc_path {
                Some(path) => Some(DbcBusOverlay::load(Path::new(path))?),
                None => None,
            };
            can_buses.insert(
                bus_spec.name.clone(),
                EnvCanBusState {
                    name: bus_spec.name.clone(),
                    vcan_iface: bus_spec.vcan_iface.clone(),
                    fd_capable,
                    bitrate,
                    bitrate_data,
                    members: bus_spec.members.clone(),
                    socket,
                    dbc,
                    latest_frames: HashMap::new(),
                    pending_delivery: Vec::new(),
                    schedules: BTreeMap::new(),
                },
            );
        }

        for shared in &env_spec.shared_channels {
            attach_shared_channel(&env_spec.name, shared).await?;
        }

        Ok(Self {
            name: env_spec.name.clone(),
            socket_path,
            tick_duration_us: tick_duration_us.unwrap_or(1),
            instances: env_spec
                .instances
                .iter()
                .map(|instance| instance.name.clone())
                .collect(),
            time: TimeEngine::default(),
            can_buses,
            instance_bus_map,
            ignored_instance_buses: HashSet::new(),
            shutdown: false,
        })
    }
}

pub async fn run_listener(socket_path: PathBuf, env_spec: EnvSpec) -> Result<(), std::io::Error> {
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let state = EnvState::bootstrap(socket_path.clone(), env_spec)
        .await
        .map_err(std::io::Error::other)?;
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(err) => {
            cleanup_listener_runtime(&state.instances, &state.socket_path, &state.name).await;
            return Err(err);
        }
    };
    if let Err(err) = std::fs::write(pid_path(&state.name), std::process::id().to_string()) {
        cleanup_listener_runtime(&state.instances, &state.socket_path, &state.name).await;
        return Err(err);
    }

    let state = Arc::new(Mutex::new(state));
    let tick_state = Arc::clone(&state);
    let tick_task = tokio::spawn(async move {
        run_tick_loop(tick_state).await;
    });

    let result = loop {
        {
            let state = state.lock().await;
            if state.shutdown {
                break Ok(());
            }
        }
        match timeout(Duration::from_millis(100), listener.accept()).await {
            Ok(Ok((stream, _))) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, state).await;
                });
            }
            Ok(Err(err)) => {
                break Err(err);
            }
            Err(_) => {}
        }
    };

    tick_task.abort();
    let (instances, socket_path, env_name) = {
        let state = state.lock().await;
        (
            state.instances.clone(),
            state.socket_path.clone(),
            state.name.clone(),
        )
    };
    cleanup_listener_runtime(&instances, &socket_path, &env_name).await;
    result
}

async fn handle_connection(
    mut stream: UnixStream,
    state: Arc<Mutex<EnvState>>,
) -> Result<(), std::io::Error> {
    let mut line = String::new();
    let mut reader = BufReader::new(&mut stream);
    let read = reader.read_line(&mut line).await?;
    if read == 0 {
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(line.trim_end()) {
        Ok(request) => {
            let id = request.id;
            let result = {
                let mut state = state.lock().await;
                dispatch_action(request.action, &mut state).await
            };
            match result {
                Ok(data) => Response::ok(id, data),
                Err(err) => Response::err(id, err),
            }
        }
        Err(err) => Response::err(uuid::Uuid::new_v4(), format!("invalid request json: {err}")),
    };
    drop(reader);
    let mut payload = serde_json::to_string(&response).unwrap_or_else(|err| {
        format!("{{\"success\":false,\"error\":\"response serialization failed: {err}\"}}")
    });
    payload.push('\n');
    stream.write_all(payload.as_bytes()).await?;
    Ok(())
}

async fn run_tick_loop(state: Arc<Mutex<EnvState>>) {
    loop {
        let sleep_duration = {
            let mut state = state.lock().await;
            if state.shutdown {
                return;
            }
            let tick_duration_us = state.tick_duration_us;
            let due_ticks = state.time.tick_realtime_due(tick_duration_us);
            // This lock currently spans per-instance RPCs inside `advance_env_ticks`.
            // If env command latency becomes noticeable under catch-up, revisit with
            // a split-state or actor-style tick loop rather than a piecemeal unlock.
            if let Err(err) = advance_env_ticks(&mut state, due_ticks).await {
                tracing::error!("env '{}' tick loop failed: {err}", state.name);
                state.shutdown = true;
                return;
            }
            state.time.realtime_poll_delay(tick_duration_us)
        };
        sleep(sleep_duration).await;
    }
}

async fn dispatch_action(action: Action, state: &mut EnvState) -> Result<ResponseData, String> {
    match action {
        Action::EnvStatus { env } => {
            ensure_env_name(state, &env)?;
            Ok(ResponseData::EnvStatus {
                env,
                running: true,
                instance_count: state.instances.len(),
                tick_duration_us: state.tick_duration_us,
            })
        }
        Action::EnvReset { env } => {
            ensure_env_name(state, &env)?;
            for instance in &state.instances {
                send_action_success(instance, Action::Reset).await?;
            }
            state.time.reset();
            Ok(ResponseData::Ack)
        }
        Action::EnvTimeStart { env } => {
            ensure_env_name(state, &env)?;
            state.time.start().map_err(|err| err.to_string())?;
            env_time_status(state)
        }
        Action::EnvTimePause { env } => {
            ensure_env_name(state, &env)?;
            state.time.pause().map_err(|err| err.to_string())?;
            env_time_status(state)
        }
        Action::EnvTimeStep { env, duration } => {
            ensure_env_name(state, &env)?;
            let duration_us = parse_duration_us(&duration).map_err(|err| err.to_string())?;
            let step = state
                .time
                .step_ticks(state.tick_duration_us, duration_us)
                .map_err(|err| err.to_string())?;
            advance_env_ticks(state, step.advanced_ticks).await?;
            Ok(ResponseData::TimeAdvanced {
                requested_us: step.requested_us,
                advanced_ticks: step.advanced_ticks,
                advanced_us: step.advanced_us,
            })
        }
        Action::EnvTimeSpeed { env, multiplier } => {
            ensure_env_name(state, &env)?;
            if let Some(multiplier) = multiplier {
                state
                    .time
                    .set_speed(multiplier)
                    .map_err(|err| err.to_string())?;
            }
            Ok(ResponseData::Speed {
                speed: state.time.speed(),
            })
        }
        Action::EnvTimeStatus { env } => {
            ensure_env_name(state, &env)?;
            env_time_status(state)
        }
        Action::EnvCanBuses { env } => {
            ensure_env_name(state, &env)?;
            let buses = state
                .can_buses
                .values()
                .enumerate()
                .map(|(idx, bus)| CanBusData {
                    id: u32::try_from(idx).unwrap_or(u32::MAX),
                    name: bus.name.clone(),
                    bitrate: bus.bitrate,
                    bitrate_data: bus.bitrate_data,
                    fd_capable: bus.fd_capable,
                    attached_iface: Some(bus.vcan_iface.clone()),
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::CanBuses { buses })
        }
        Action::EnvCanLoadDbc {
            env,
            bus_name,
            path,
        } => {
            ensure_env_name(state, &env)?;
            let bus = state
                .can_buses
                .get_mut(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            let overlay = DbcBusOverlay::load(Path::new(&path))?;
            let signal_count = overlay.signal_names().count();
            bus.dbc = Some(overlay);
            Ok(ResponseData::DbcLoaded {
                bus: bus_name,
                signal_count,
            })
        }
        Action::EnvCanSend {
            env,
            bus_name,
            arb_id,
            data_hex,
            flags,
        } => {
            ensure_env_name(state, &env)?;
            let frame = parse_env_frame(state, &bus_name, arb_id, &data_hex, flags.unwrap_or(0))?;
            queue_env_frame(state, &bus_name, None, &frame)?;
            Ok(ResponseData::CanSend {
                bus: bus_name,
                arb_id,
                len: frame.len,
            })
        }
        Action::EnvCanInspect { env, bus_name } => {
            ensure_env_name(state, &env)?;
            let bus = state
                .can_buses
                .get(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            let mut frames = bus.latest_frames.values().cloned().collect::<Vec<_>>();
            frames.sort_by(|lhs, rhs| lhs.arb_id.cmp(&rhs.arb_id));
            Ok(ResponseData::CanInspect {
                bus: bus_name,
                frames: frames.iter().map(frame_data).collect(),
            })
        }
        Action::EnvCanScheduleAdd {
            env,
            bus_name,
            job_id,
            arb_id,
            data_hex,
            every,
            flags,
        } => {
            ensure_env_name(state, &env)?;
            let every_ticks = duration_to_env_ticks(state.tick_duration_us, &every)?;
            let frame = parse_env_frame(state, &bus_name, arb_id, &data_hex, flags.unwrap_or(0))?;
            let job_id = job_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let next_due_tick = state.time.status(state.tick_duration_us).elapsed_ticks;
            let schedule = CanScheduleJob {
                job_id: job_id.clone(),
                arb_id,
                flags: frame.flags,
                data_hex,
                frame,
                every_ticks,
                next_due_tick,
                enabled: true,
            };
            ensure_unique_schedule_job_id(
                state.can_buses.values().map(|bus| &bus.schedules),
                &job_id,
            )?;
            let bus = state
                .can_buses
                .get_mut(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            bus.schedules.insert(job_id, schedule);
            Ok(ResponseData::Ack)
        }
        Action::EnvCanScheduleUpdate {
            env,
            job_id,
            arb_id,
            data_hex,
            every,
            flags,
        } => {
            ensure_env_name(state, &env)?;
            let every_ticks = duration_to_env_ticks(state.tick_duration_us, &every)?;
            let bus_name = locate_schedule_bus(state, &job_id)?;
            let frame = parse_env_frame(state, &bus_name, arb_id, &data_hex, flags.unwrap_or(0))?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            update_schedule(schedule, arb_id, data_hex, frame, every_ticks);
            Ok(ResponseData::Ack)
        }
        Action::EnvCanScheduleRemove { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (bus_name, _) = locate_schedule_mut(state, &job_id)?;
            let bus = state
                .can_buses
                .get_mut(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            bus.schedules.remove(&job_id);
            Ok(ResponseData::Ack)
        }
        Action::EnvCanScheduleStop { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            schedule.enabled = false;
            Ok(ResponseData::Ack)
        }
        Action::EnvCanScheduleStart { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            start_schedule(schedule);
            Ok(ResponseData::Ack)
        }
        Action::EnvCanScheduleList { env, bus_name } => {
            ensure_env_name(state, &env)?;
            let schedules = state
                .can_buses
                .iter()
                .filter(|(name, _)| bus_name.as_ref().is_none_or(|requested| requested == *name))
                .flat_map(|(name, bus)| {
                    bus.schedules.values().map(|schedule| CanScheduleData {
                        job_id: schedule.job_id.clone(),
                        bus: name.clone(),
                        arb_id: schedule.arb_id,
                        data_hex: schedule.data_hex.clone(),
                        flags: schedule.flags,
                        every_ticks: schedule.every_ticks,
                        next_due_tick: schedule.next_due_tick,
                        enabled: schedule.enabled,
                    })
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::CanSchedules { schedules })
        }
        Action::EnvClose { env } => {
            ensure_env_name(state, &env)?;
            state.shutdown = true;
            Ok(ResponseData::Ack)
        }
        other => Err(format!("unsupported env action: {other:?}")),
    }
}

async fn advance_env_ticks(state: &mut EnvState, ticks: u64) -> Result<(), String> {
    for _ in 0..ticks {
        advance_single_tick(state).await?;
    }
    Ok(())
}

async fn advance_single_tick(state: &mut EnvState) -> Result<(), String> {
    let mut instance_rx: HashMap<String, HashMap<String, Vec<SimCanFrame>>> = HashMap::new();
    let current_tick = state.time.status(state.tick_duration_us).elapsed_ticks;

    for bus in state.can_buses.values_mut() {
        let mut ready_frames = std::mem::take(&mut bus.pending_delivery);
        for frame in bus.socket.recv_all()? {
            ready_frames.push(PendingFrame {
                source_instance: None,
                frame,
            });
        }
        for schedule in bus.schedules.values_mut() {
            if !schedule.enabled || schedule.next_due_tick > current_tick {
                continue;
            }
            bus.socket.send(&schedule.frame)?;
            bus.latest_frames.insert(
                frame_key_from_frame(&schedule.frame),
                schedule.frame.clone(),
            );
            ready_frames.push(PendingFrame {
                source_instance: None,
                frame: schedule.frame.clone(),
            });
            schedule.next_due_tick = current_tick.saturating_add(schedule.every_ticks.max(1));
        }

        for pending in ready_frames {
            bus.latest_frames
                .insert(frame_key_from_frame(&pending.frame), pending.frame.clone());
            for member in &bus.members {
                if pending
                    .source_instance
                    .as_ref()
                    .is_some_and(|source| source == &member.instance_name)
                {
                    continue;
                }
                instance_rx
                    .entry(member.instance_name.clone())
                    .or_default()
                    .entry(member.bus_name.clone())
                    .or_default()
                    .push(pending.frame.clone());
            }
        }
    }

    let instances = state.instances.clone();
    for instance in instances {
        let can_rx = instance_rx
            .remove(&instance)
            .unwrap_or_default()
            .into_iter()
            .map(|(bus_name, frames)| CanBusFramesData {
                bus_name,
                frames: frames.iter().map(Into::into).collect(),
            })
            .collect::<Vec<_>>();
        let response = send_request(
            &instance,
            &Request {
                id: uuid::Uuid::new_v4(),
                action: Action::WorkerStep { can_rx },
            },
        )
        .await
        .map_err(|err| err.to_string())?;
        let ResponseData::WorkerStep { can_tx } = response
            .data
            .ok_or_else(|| format!("missing worker-step payload for instance '{instance}'"))?
        else {
            return Err(format!(
                "unexpected worker-step payload while stepping instance '{instance}'"
            ));
        };
        for batch in can_tx {
            let Some(env_bus_name) =
                resolve_env_bus_name(state, instance.as_str(), batch.bus_name.as_str())
            else {
                continue;
            };
            for frame in batch
                .frames
                .into_iter()
                .map(SimCanFrame::try_from)
                .collect::<Result<Vec<_>, _>>()?
            {
                queue_env_frame(state, &env_bus_name, Some(instance.clone()), &frame)?;
            }
        }
    }

    state.time.advance_ticks(1);
    Ok(())
}

fn resolve_env_bus_name(state: &mut EnvState, instance: &str, bus_name: &str) -> Option<String> {
    let key = (instance.to_string(), bus_name.to_string());
    if let Some(env_bus_name) = state.instance_bus_map.get(&key) {
        return Some(env_bus_name.clone());
    }

    if state.ignored_instance_buses.insert(key) {
        tracing::warn!(
            "instance '{}:{}' emitted CAN traffic on an unmapped bus; dropping frames",
            instance,
            bus_name
        );
    }
    None
}

async fn attach_shared_channel(
    env_name: &str,
    shared: &EnvSharedChannelSpec,
) -> Result<(), String> {
    let shared_root = crate::daemon::lifecycle::session_root().join("shared");
    std::fs::create_dir_all(&shared_root).map_err(|err| {
        format!(
            "failed to create shared region root '{}': {err}",
            shared_root.display()
        )
    })?;
    let region_path = shared_root.join(format!("{env_name}__{}.bin", shared.name));
    let region_path_str = region_path.display().to_string();

    if !shared
        .members
        .iter()
        .any(|member| member.instance_name == shared.writer_instance)
    {
        return Err(format!(
            "shared channel '{}' writer '{}' is not listed in members",
            shared.name, shared.writer_instance
        ));
    }

    for member in &shared.members {
        send_action_success(
            &member.instance_name,
            Action::SharedAttach {
                channel_name: member.channel_name.clone(),
                path: region_path_str.clone(),
                writer: member.instance_name == shared.writer_instance,
                writer_session: shared.writer_instance.clone(),
            },
        )
        .await?;
    }
    Ok(())
}

fn ensure_env_name(state: &EnvState, requested: &str) -> Result<(), String> {
    if state.name == requested {
        Ok(())
    } else {
        Err(format!(
            "env daemon '{}' cannot service requests for env '{}'",
            state.name, requested
        ))
    }
}

fn duration_to_env_ticks(tick_duration_us: u32, raw: &str) -> Result<u64, String> {
    let duration_us = parse_duration_us(raw).map_err(|err| err.to_string())?;
    if duration_us == 0 {
        return Err("schedule period must be greater than zero".to_string());
    }
    let tick = u64::from(tick_duration_us.max(1));
    Ok((duration_us / tick).max(1))
}

fn parse_env_frame(
    state: &EnvState,
    bus_name: &str,
    arb_id: u32,
    data_hex: &str,
    flags: u8,
) -> Result<SimCanFrame, String> {
    let payload = parse_data_hex(data_hex)?;
    let mut data = [0_u8; 64];
    data[..payload.len()].copy_from_slice(&payload);
    let frame = SimCanFrame {
        arb_id,
        len: payload.len() as u8,
        flags,
        data,
    };
    validate_env_frame(state, bus_name, &frame)?;
    Ok(frame)
}

fn queue_env_frame(
    state: &mut EnvState,
    bus_name: &str,
    source_instance: Option<String>,
    frame: &SimCanFrame,
) -> Result<(), String> {
    validate_env_frame(state, bus_name, frame)?;
    let bus = state
        .can_buses
        .get_mut(bus_name)
        .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
    bus.socket.send(frame)?;
    bus.latest_frames
        .insert(frame_key_from_frame(frame), frame.clone());
    bus.pending_delivery.push(PendingFrame {
        source_instance,
        frame: frame.clone(),
    });
    Ok(())
}

fn validate_env_frame(state: &EnvState, bus_name: &str, frame: &SimCanFrame) -> Result<(), String> {
    let bus = state
        .can_buses
        .get(bus_name)
        .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
    validate_can_frame(&bus.name, bus.fd_capable, frame)
}

fn locate_schedule_mut<'a>(
    state: &'a mut EnvState,
    job_id: &str,
) -> Result<(String, &'a mut CanScheduleJob), String> {
    for (bus_name, bus) in &mut state.can_buses {
        if let Some(schedule) = bus.schedules.get_mut(job_id) {
            return Ok((bus_name.clone(), schedule));
        }
    }
    Err(format!("CAN schedule '{job_id}' not found"))
}

fn locate_schedule_bus(state: &EnvState, job_id: &str) -> Result<String, String> {
    state
        .can_buses
        .iter()
        .find(|(_, bus)| bus.schedules.contains_key(job_id))
        .map(|(bus_name, _)| bus_name.clone())
        .ok_or_else(|| format!("CAN schedule '{job_id}' not found"))
}

async fn cleanup_listener_runtime(instances: &[String], socket_path: &Path, env_name: &str) {
    shutdown_instances(instances).await;
    if socket_path.exists() {
        let _ = std::fs::remove_file(socket_path);
    }
    let pid = pid_path(env_name);
    if pid.exists() {
        let _ = std::fs::remove_file(pid);
    }
}

fn ensure_unique_schedule_job_id<'a, I>(schedules: I, job_id: &str) -> Result<(), String>
where
    I: IntoIterator<Item = &'a BTreeMap<String, CanScheduleJob>>,
{
    if schedules
        .into_iter()
        .any(|schedule_map| schedule_map.contains_key(job_id))
    {
        return Err(format!("CAN schedule '{job_id}' already exists"));
    }
    Ok(())
}

fn frame_data(frame: &SimCanFrame) -> CanFrameData {
    CanFrameData {
        arb_id: frame.arb_id,
        len: frame.len,
        flags: frame.flags,
        data_hex: frame
            .payload()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn env_time_status(state: &EnvState) -> Result<ResponseData, String> {
    let status = state.time.status(state.tick_duration_us);
    Ok(ResponseData::TimeStatus {
        state: status.state,
        elapsed_ticks: status.elapsed_ticks,
        elapsed_time_us: status.elapsed_time_us,
        speed: status.speed,
    })
}

async fn send_action_success(instance: &str, action: Action) -> Result<(), String> {
    let response = send_request(
        instance,
        &Request {
            id: uuid::Uuid::new_v4(),
            action,
        },
    )
    .await
    .map_err(|err| err.to_string())?;
    if response.success {
        Ok(())
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "command failed".to_string()))
    }
}

fn update_schedule(
    schedule: &mut CanScheduleJob,
    arb_id: u32,
    data_hex: String,
    frame: SimCanFrame,
    every_ticks: u64,
) {
    schedule.arb_id = arb_id;
    schedule.flags = frame.flags;
    schedule.data_hex = data_hex;
    schedule.frame = frame;
    schedule.every_ticks = every_ticks;
}

fn start_schedule(schedule: &mut CanScheduleJob) {
    schedule.enabled = true;
}

async fn run_bootstrap_instance_command(
    exe: &Path,
    instance_name: &str,
    spec_path: &Path,
) -> Result<std::process::Output, String> {
    tokio::process::Command::new(exe)
        .arg("--bootstrap-instance")
        .arg("--instance")
        .arg(instance_name)
        .arg("--load-spec-path")
        .arg(spec_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|err| format!("failed to bootstrap instance '{instance_name}': {err}"))
}

async fn bootstrap_instance_detached(instance: &EnvInstanceSpec) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    bootstrap_instance_detached_with_exe(instance, &exe).await
}

async fn bootstrap_instance_detached_with_exe(
    instance: &EnvInstanceSpec,
    exe: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(crate::daemon::lifecycle::bootstrap_dir())
        .map_err(|err| format!("failed to create bootstrap dir: {err}"))?;
    let spec_path = crate::daemon::lifecycle::bootstrap_dir().join(format!(
        "{}-helper-{}.json",
        instance.name,
        uuid::Uuid::new_v4()
    ));
    write_load_spec(&spec_path, &instance.load_spec).map_err(|err| err.to_string())?;

    let output = run_bootstrap_instance_command(exe, &instance.name, &spec_path).await;
    let _ = std::fs::remove_file(&spec_path);
    let output = output?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "failed to bootstrap instance '{}': {}",
            instance.name,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

async fn rollback_instances(started_instances: &[String]) {
    shutdown_instances(started_instances).await;
}

async fn shutdown_instances(instances: &[String]) {
    for instance in instances {
        if send_action_success(instance, Action::Close).await.is_err()
            && let Some(pid) = read_pid(instance)
        {
            let _ = kill_pid(pid);
        }
    }
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
        let pair = format!("{}{}", bytes[idx] as char, bytes[idx + 1] as char);
        let value = u8::from_str_radix(&pair, 16)
            .map_err(|_| format!("invalid CAN payload hex '{raw}': bad byte '{pair}'"))?;
        payload.push(value);
        idx += 2;
    }
    Ok(payload)
}

fn validate_can_frame(bus_name: &str, fd_capable: bool, frame: &SimCanFrame) -> Result<(), String> {
    if (frame.flags & CAN_FLAG_RESERVED_MASK) != 0 {
        return Err(format!(
            "CAN frame for bus '{}' has reserved flag bits set",
            bus_name
        ));
    }
    if (frame.flags & CAN_FLAG_EXTENDED) != 0 {
        if frame.arb_id > 0x1FFF_FFFF {
            return Err(format!(
                "CAN frame for bus '{}' has invalid extended arbitration id 0x{:X}",
                bus_name, frame.arb_id
            ));
        }
    } else if frame.arb_id > 0x7FF {
        return Err(format!(
            "CAN frame for bus '{}' has invalid standard arbitration id 0x{:X}",
            bus_name, frame.arb_id
        ));
    }
    if frame.len > 64 {
        return Err(format!(
            "CAN frame for bus '{}' has invalid payload length {}",
            bus_name, frame.len
        ));
    }

    let fd_requested =
        (frame.flags & CAN_FLAG_FD) != 0 || (frame.flags & (CAN_FLAG_BRS | CAN_FLAG_ESI)) != 0;
    if fd_requested {
        if !fd_capable {
            return Err(format!(
                "CAN bus '{}' is classic-only and cannot carry FD frames",
                bus_name
            ));
        }
        if !matches!(frame.len, 0..=8 | 12 | 16 | 20 | 24 | 32 | 48 | 64) {
            return Err(format!(
                "CAN FD frame for bus '{}' has invalid length {}; valid lengths are 0-8,12,16,20,24,32,48,64",
                bus_name, frame.len
            ));
        }
        if (frame.flags & CAN_FLAG_RTR) != 0 {
            return Err(format!(
                "CAN FD frame for bus '{}' cannot set RTR flag",
                bus_name
            ));
        }
    } else if frame.len > 8 {
        return Err(format!(
            "classic CAN frame for bus '{}' has invalid length {}",
            bus_name, frame.len
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::LoadSpec;
    use crate::protocol::CanFrameWireData;
    use serial_test::serial;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    fn frame(arb_id: u32, flags: u8, data: &[u8]) -> SimCanFrame {
        let mut payload = [0_u8; 64];
        payload[..data.len()].copy_from_slice(data);
        SimCanFrame {
            arb_id,
            len: data.len() as u8,
            flags,
            data: payload,
        }
    }

    fn schedule(enabled: bool) -> CanScheduleJob {
        let original_frame = frame(0x123, 0, &[0xAA, 0xBB]);
        CanScheduleJob {
            job_id: "job-1".to_string(),
            arb_id: original_frame.arb_id,
            flags: original_frame.flags,
            data_hex: "AABB".to_string(),
            frame: original_frame,
            every_ticks: 10,
            next_due_tick: 5,
            enabled,
        }
    }

    fn restore_agent_sim_home(original_home: Option<std::ffi::OsString>) {
        if let Some(value) = original_home {
            unsafe {
                std::env::set_var("AGENT_SIM_HOME", value);
            }
        } else {
            unsafe {
                std::env::remove_var("AGENT_SIM_HOME");
            }
        }
    }

    #[test]
    fn schedule_update_preserves_disabled_state() {
        let mut schedule = schedule(false);
        let updated_frame = frame(0x456, CAN_FLAG_EXTENDED, &[0x01, 0x02, 0x03]);

        update_schedule(
            &mut schedule,
            updated_frame.arb_id,
            "010203".to_string(),
            updated_frame,
            42,
        );

        assert_eq!(schedule.arb_id, 0x456);
        assert_eq!(schedule.flags, CAN_FLAG_EXTENDED);
        assert_eq!(schedule.data_hex, "010203");
        assert_eq!(schedule.every_ticks, 42);
        assert!(!schedule.enabled);
        assert_eq!(schedule.frame.len, 3);
        assert_eq!(schedule.frame.payload(), &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn schedule_update_preserves_enabled_state() {
        let mut schedule = schedule(true);
        let updated_frame = frame(0x456, CAN_FLAG_EXTENDED, &[0x01, 0x02, 0x03]);

        update_schedule(
            &mut schedule,
            updated_frame.arb_id,
            "010203".to_string(),
            updated_frame,
            42,
        );

        assert!(schedule.enabled);
        assert_eq!(schedule.frame.payload(), &[0x01, 0x02, 0x03]);
    }

    #[test]
    fn start_schedule_reenables_stopped_schedule() {
        let mut schedule = schedule(false);
        start_schedule(&mut schedule);

        assert!(schedule.enabled);
    }

    #[test]
    fn schedule_job_ids_must_be_unique_across_buses() {
        let mut bus_a = BTreeMap::new();
        let bus_b = BTreeMap::new();
        bus_a.insert("job-1".to_string(), schedule(true));

        let err = ensure_unique_schedule_job_id([&bus_a, &bus_b], "job-1").unwrap_err();

        assert_eq!(err, "CAN schedule 'job-1' already exists");
    }

    #[test]
    fn schedule_job_id_check_allows_new_ids() {
        let mut bus_a = BTreeMap::new();
        let bus_b = BTreeMap::new();
        bus_a.insert("job-1".to_string(), schedule(true));

        let result = ensure_unique_schedule_job_id([&bus_a, &bus_b], "job-2");

        assert!(result.is_ok());
    }

    #[test]
    fn resolve_env_bus_name_skips_unmapped_bus_once() {
        let mut state = EnvState {
            name: "env".to_string(),
            socket_path: PathBuf::new(),
            tick_duration_us: 20,
            instances: vec!["instance-a".to_string()],
            time: TimeEngine::default(),
            can_buses: BTreeMap::new(),
            instance_bus_map: HashMap::from([(
                ("instance-a".to_string(), "external".to_string()),
                "env-external".to_string(),
            )]),
            ignored_instance_buses: HashSet::new(),
            shutdown: false,
        };

        let mapped = resolve_env_bus_name(&mut state, "instance-a", "external");
        assert_eq!(mapped.as_deref(), Some("env-external"));
        assert!(state.ignored_instance_buses.is_empty());

        let first_unmapped = resolve_env_bus_name(&mut state, "instance-a", "internal");
        let second_unmapped = resolve_env_bus_name(&mut state, "instance-a", "internal");
        assert!(first_unmapped.is_none());
        assert!(second_unmapped.is_none());
        assert_eq!(state.ignored_instance_buses.len(), 1);
        assert!(
            state
                .ignored_instance_buses
                .contains(&("instance-a".to_string(), "internal".to_string()))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn advance_single_tick_ignores_unmapped_instance_bus() {
        let home = tempfile::tempdir().expect("temp AGENT_SIM_HOME should be creatable");
        let original_home = std::env::var_os("AGENT_SIM_HOME");
        unsafe {
            std::env::set_var("AGENT_SIM_HOME", home.path());
        }

        let instance = "instance-a";
        let socket_path = crate::daemon::lifecycle::socket_path(instance);
        std::fs::create_dir_all(
            socket_path
                .parent()
                .expect("instance socket should have a parent directory"),
        )
        .expect("instance socket parent should be creatable");
        let listener =
            UnixListener::bind(&socket_path).expect("fake instance listener should bind");
        let server = tokio::spawn(async move {
            let (_probe_stream, _) = listener
                .accept()
                .await
                .expect("fake instance should accept readiness probe");
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("fake instance should accept worker-step request");
            let mut line = String::new();
            let mut reader = BufReader::new(&mut stream);
            reader
                .read_line(&mut line)
                .await
                .expect("request should be readable");
            drop(reader);
            let request: Request =
                serde_json::from_str(line.trim_end()).expect("request json should parse");
            assert!(matches!(request.action, Action::WorkerStep { .. }));
            let response = Response::ok(
                request.id,
                ResponseData::WorkerStep {
                    can_tx: vec![CanBusFramesData {
                        bus_name: "internal".to_string(),
                        frames: vec![CanFrameWireData {
                            arb_id: 0x321,
                            len: 2,
                            flags: 0,
                            data: vec![0xAB, 0xCD],
                        }],
                    }],
                },
            );
            let mut payload = serde_json::to_string(&response).expect("response should serialize");
            payload.push('\n');
            stream
                .write_all(payload.as_bytes())
                .await
                .expect("response should be writable");
        });

        let mut state = EnvState {
            name: "env".to_string(),
            socket_path: PathBuf::new(),
            tick_duration_us: 20,
            instances: vec![instance.to_string()],
            time: TimeEngine::default(),
            can_buses: BTreeMap::new(),
            instance_bus_map: HashMap::new(),
            ignored_instance_buses: HashSet::new(),
            shutdown: false,
        };

        advance_single_tick(&mut state)
            .await
            .expect("unmapped instance buses should be ignored");
        server.await.expect("fake instance task should finish");

        assert_eq!(state.time.status(state.tick_duration_us).elapsed_ticks, 1);
        assert_eq!(state.ignored_instance_buses.len(), 1);
        assert!(
            state
                .ignored_instance_buses
                .contains(&(instance.to_string(), "internal".to_string()))
        );

        restore_agent_sim_home(original_home);
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn bootstrap_instance_detached_removes_temp_file_when_spawn_fails() {
        let home = tempfile::tempdir().expect("temp AGENT_SIM_HOME should be creatable");
        let original_home = std::env::var_os("AGENT_SIM_HOME");
        unsafe {
            std::env::set_var("AGENT_SIM_HOME", home.path());
        }

        let instance = EnvInstanceSpec {
            name: "instance-a".to_string(),
            load_spec: LoadSpec {
                libpath: "/tmp/fake-lib.so".to_string(),
                env_tag: Some("env-a".to_string()),
                flash: Vec::new(),
            },
        };
        let missing_exe = home.path().join("missing-bootstrap-binary");
        let err = bootstrap_instance_detached_with_exe(&instance, &missing_exe)
            .await
            .expect_err("missing bootstrap binary should fail");
        assert!(
            err.contains("failed to bootstrap instance 'instance-a'"),
            "unexpected error: {err}"
        );

        let bootstrap_dir = crate::daemon::lifecycle::bootstrap_dir();
        let entries = std::fs::read_dir(&bootstrap_dir)
            .expect("bootstrap dir should exist")
            .collect::<Result<Vec<_>, _>>()
            .expect("bootstrap dir should be readable");
        assert!(
            entries.is_empty(),
            "temp load specs should be cleaned up on spawn failure"
        );

        restore_agent_sim_home(original_home);
    }
}
