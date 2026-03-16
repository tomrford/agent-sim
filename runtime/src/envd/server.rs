#[path = "server/bootstrap.rs"]
mod bootstrap;
#[path = "server/dispatch.rs"]
mod dispatch;
#[path = "server/instance_worker.rs"]
mod instance_worker;
#[path = "server/tick.rs"]
mod tick;

use crate::can::CanSocket;
use crate::can::dbc::{DbcBusOverlay, frame_key_from_frame};
use crate::daemon::lifecycle::{kill_pid, read_pid};
use crate::envd::lifecycle::pid_path;
use crate::envd::spec::EnvSpec;
use crate::ipc::{self, BoxedLocalStream};
use crate::protocol::{
    CanFrameData, InstanceAction, Request, RequestAction, Response, parse_duration_us,
};
use crate::signal_selectors::EnvSignalCatalog;
use crate::sim::time::TimeEngine;
#[cfg(test)]
use crate::sim::types::CAN_FLAG_EXTENDED;
use crate::sim::types::SimCanFrame;
use crate::trace::CsvTraceWriter;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, split};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::yield_now;
use tokio::time::sleep;

const MAX_ENV_ACTIONS_PER_TURN: usize = 16;
const MAX_ENV_TICKS_PER_TURN: u64 = 64;

struct EnvState {
    name: String,
    socket_path: PathBuf,
    tick_duration_us: u32,
    instances: Vec<String>,
    signal_catalog: EnvSignalCatalog,
    instance_workers: HashMap<String, instance_worker::InstanceWorker>,
    time: TimeEngine,
    realtime_tick_backlog: u64,
    can_buses: BTreeMap<String, EnvCanBusState>,
    trace: EnvTraceState,
    shutdown: bool,
}

struct EnvCanBusState {
    name: String,
    vcan_iface: String,
    fd_capable: bool,
    bitrate: u32,
    bitrate_data: u32,
    socket: CanSocket,
    dbc: Option<DbcBusOverlay>,
    latest_frames: HashMap<u32, SimCanFrame>,
    schedules: BTreeMap<String, CanScheduleJob>,
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

struct EnvTraceState {
    active: Option<ActiveEnvTrace>,
    last_path: Option<PathBuf>,
    last_row_count: u64,
    last_signal_count: usize,
    last_period_us: Option<u64>,
}

struct ActiveEnvTrace {
    writer: CsvTraceWriter,
    period_ticks: u64,
    period_us: u64,
    next_due_tick: u64,
    signals: Vec<EnvTraceSignal>,
}

#[derive(Clone)]
struct EnvTraceSignal {
    instance: String,
    local_id: u32,
    name: String,
}

struct ActionMessage {
    request: Request,
    response_tx: oneshot::Sender<Response>,
}

pub async fn run_listener(socket_path: PathBuf, env_spec: EnvSpec) -> Result<(), std::io::Error> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let state = EnvState::bootstrap(socket_path.clone(), env_spec)
        .await
        .map_err(std::io::Error::other)?;
    let mut listener = match ipc::bind_listener(&socket_path).await {
        Ok(listener) => listener,
        Err(err) => {
            cleanup_listener_runtime(&state).await;
            return Err(err);
        }
    };
    if let Err(err) = ipc::create_endpoint_marker(&socket_path) {
        cleanup_listener_runtime(&state).await;
        return Err(err);
    }
    if let Err(err) = std::fs::write(pid_path(&state.name), std::process::id().to_string()) {
        cleanup_listener_runtime(&state).await;
        return Err(err);
    }

    let (action_tx, action_rx) = mpsc::channel::<ActionMessage>(256);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let actor_task = tokio::spawn(run_actor_task(state, action_rx, shutdown_tx));

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
                        tokio::spawn(async move {
                            let _ = handle_connection(stream, action_tx).await;
                        });
                    }
                    Err(err) => {
                        listener_error = Some(err);
                        break;
                    }
                }
            }
        }
    }

    drop(action_tx);
    let state = actor_task
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    cleanup_listener_runtime(&state).await;
    if let Some(err) = listener_error {
        return Err(err);
    }
    Ok(())
}

async fn handle_connection(
    stream: BoxedLocalStream,
    action_tx: mpsc::Sender<ActionMessage>,
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
                if action_tx
                    .send(ActionMessage {
                        request,
                        response_tx,
                    })
                    .await
                    .is_err()
                {
                    Response::err(request_id, "env daemon unavailable")
                } else {
                    match response_rx.await {
                        Ok(response) => response,
                        Err(_) => Response::err(request_id, "env daemon unavailable"),
                    }
                }
            }
            Err(err) => Response::err(uuid::Uuid::new_v4(), format!("invalid request json: {err}")),
        };
        let mut payload = serde_json::to_string(&response).unwrap_or_else(|err| {
            format!("{{\"success\":false,\"error\":\"response serialization failed: {err}\"}}")
        });
        payload.push('\n');
        write_half.write_all(payload.as_bytes()).await?;
    }
}

async fn process_action_message(message: ActionMessage, state: &mut EnvState) {
    let response = handle_action(message.request, state).await;
    let _ = message.response_tx.send(response);
}

async fn handle_action(request: Request, state: &mut EnvState) -> Response {
    let id = request.id;
    let result = match request.action {
        RequestAction::Env(action) => dispatch::dispatch_action(action, state).await,
        RequestAction::Instance(_) | RequestAction::Worker(_) => {
            Err("instance-owned action sent to env daemon".to_string())
        }
    };

    match result {
        Ok(data) => Response::ok(id, data),
        Err(err) => Response::err(id, err),
    }
}

async fn run_actor_task(
    mut state: EnvState,
    mut action_rx: mpsc::Receiver<ActionMessage>,
    shutdown_tx: watch::Sender<bool>,
) -> EnvState {
    loop {
        process_env_action_batch(&mut action_rx, &mut state, MAX_ENV_ACTIONS_PER_TURN).await;

        if state.shutdown {
            break;
        }

        state.realtime_tick_backlog = state
            .realtime_tick_backlog
            .saturating_add(state.time.tick_realtime_due(state.tick_duration_us));
        let tick_batch = state.realtime_tick_backlog.min(MAX_ENV_TICKS_PER_TURN);
        if let Err(err) = tick::advance_due_ticks(&mut state, tick_batch).await {
            tracing::error!("env '{}' tick loop failed: {err}", state.name);
            state.shutdown = true;
        } else {
            state.realtime_tick_backlog = state.realtime_tick_backlog.saturating_sub(tick_batch);
        }

        if state.shutdown {
            break;
        }

        if state.realtime_tick_backlog > 0 {
            yield_now().await;
            continue;
        }

        let sleep_duration = state.time.realtime_poll_delay(state.tick_duration_us);
        tokio::select! {
            received = action_rx.recv() => match received {
                Some(message) => process_action_message(message, &mut state).await,
                None => break,
            },
            _ = sleep(sleep_duration) => {}
        }
    }

    let _ = shutdown_tx.send(true);
    state
}

async fn process_env_action_batch(
    action_rx: &mut mpsc::Receiver<ActionMessage>,
    state: &mut EnvState,
    max_actions: usize,
) {
    for _ in 0..max_actions {
        let Ok(message) = action_rx.try_recv() else {
            break;
        };
        process_action_message(message, state).await;
        if state.shutdown {
            break;
        }
    }
}

fn duration_to_env_ticks(tick_duration_us: u32, raw: &str) -> Result<u64, String> {
    let duration_us = parse_duration_us(raw).map_err(|err| err.to_string())?;
    if duration_us == 0 {
        return Err("schedule period must be greater than zero".to_string());
    }
    if tick_duration_us == 0 {
        return Err("env tick duration must be greater than zero".to_string());
    }
    let tick = u64::from(tick_duration_us);
    Ok(duration_us.div_ceil(tick))
}

fn reset_env_can_state(state: &mut EnvState) {
    for bus in state.can_buses.values_mut() {
        let _ = bus.socket.recv_all();
        bus.latest_frames.clear();
        for schedule in bus.schedules.values_mut() {
            schedule.next_due_tick = 0;
        }
    }
}

fn parse_env_frame(
    state: &EnvState,
    bus_name: &str,
    arb_id: u32,
    data_hex: &str,
    flags: u8,
) -> Result<SimCanFrame, String> {
    let payload = crate::can::parse_data_hex(data_hex)?;
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

fn send_env_frame(state: &mut EnvState, bus_name: &str, frame: &SimCanFrame) -> Result<(), String> {
    let bus = state
        .can_buses
        .get_mut(bus_name)
        .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
    bus.socket.send(frame)?;
    record_env_frame(bus, frame);
    Ok(())
}

fn observe_env_bus_frames(state: &mut EnvState) -> Result<(), String> {
    for bus in state.can_buses.values_mut() {
        for frame in bus.socket.recv_all()? {
            record_env_frame(bus, &frame);
        }
    }
    Ok(())
}

fn record_env_frame(bus: &mut EnvCanBusState, frame: &SimCanFrame) {
    bus.latest_frames
        .insert(frame_key_from_frame(frame), frame.clone());
}

fn validate_env_frame(state: &EnvState, bus_name: &str, frame: &SimCanFrame) -> Result<(), String> {
    let bus = state
        .can_buses
        .get(bus_name)
        .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
    crate::can::validate_frame(&bus.name, bus.fd_capable, frame)
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

async fn cleanup_listener_runtime(state: &EnvState) {
    shutdown_instances(state).await;
    ipc::cleanup_endpoint(&state.socket_path);
    let pid = pid_path(&state.name);
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

fn update_schedule(
    schedule: &mut CanScheduleJob,
    arb_id: u32,
    data_hex: String,
    frame: SimCanFrame,
    every_ticks: u64,
    current_tick: u64,
) {
    schedule.arb_id = arb_id;
    schedule.flags = frame.flags;
    schedule.data_hex = data_hex;
    schedule.frame = frame;
    schedule.every_ticks = every_ticks;
    schedule.next_due_tick = current_tick;
}

fn start_schedule(schedule: &mut CanScheduleJob) {
    schedule.enabled = true;
}

async fn shutdown_instances(state: &EnvState) {
    let mut pending = Vec::with_capacity(state.instances.len());
    for instance in &state.instances {
        if let Some(worker) = state.instance_workers.get(instance)
            && let Ok(response_rx) = worker.begin_instance_request(InstanceAction::Close).await
        {
            pending.push((instance.clone(), response_rx));
            continue;
        }
        if let Some(pid) = read_pid(instance) {
            let _ = kill_pid(pid);
        }
    }

    for (instance, response_rx) in pending {
        if response_rx.await.ok().and_then(Result::ok).is_none()
            && let Some(pid) = read_pid(&instance)
        {
            let _ = kill_pid(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envd::spec::EnvInstanceSpec;
    use crate::ipc::LocalListener;
    use crate::load::LoadSpec;
    use crate::protocol::{EnvAction, InstanceAction, RequestAction, ResponseData, WorkerAction};
    use serial_test::serial;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::time::Duration;
    use uuid::Uuid;

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
            17,
        );

        assert_eq!(schedule.arb_id, 0x456);
        assert_eq!(schedule.flags, CAN_FLAG_EXTENDED);
        assert_eq!(schedule.data_hex, "010203");
        assert_eq!(schedule.every_ticks, 42);
        assert_eq!(schedule.next_due_tick, 17);
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
            23,
        );

        assert!(schedule.enabled);
        assert_eq!(schedule.next_due_tick, 23);
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
    fn schedule_period_rounds_up_to_avoid_running_faster_than_requested() {
        assert_eq!(
            duration_to_env_ticks(20, "30us").expect("schedule period should convert"),
            2
        );
        assert_eq!(
            duration_to_env_ticks(20, "40us").expect("schedule period should convert"),
            2
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_env_action_batch_limits_control_plane_work_per_turn() {
        let (action_tx, mut action_rx) = mpsc::channel(8);
        let mut responses = Vec::new();
        for _ in 0..4 {
            let (response_tx, response_rx) = oneshot::channel();
            action_tx
                .send(ActionMessage {
                    request: Request {
                        id: Uuid::new_v4(),
                        action: RequestAction::Env(EnvAction::TimeStatus {
                            env: "env".to_string(),
                        }),
                    },
                    response_tx,
                })
                .await
                .expect("action should enqueue");
            responses.push(response_rx);
        }

        let mut state = EnvState {
            name: "env".to_string(),
            socket_path: PathBuf::new(),
            tick_duration_us: 20,
            instances: Vec::new(),
            signal_catalog: EnvSignalCatalog::default(),
            instance_workers: HashMap::new(),
            time: TimeEngine::default(),
            realtime_tick_backlog: 0,
            can_buses: BTreeMap::new(),
            trace: EnvTraceState {
                active: None,
                last_path: None,
                last_row_count: 0,
                last_signal_count: 0,
                last_period_us: None,
            },
            shutdown: false,
        };

        process_env_action_batch(&mut action_rx, &mut state, 2).await;

        for response in responses.iter_mut().take(2) {
            assert!(
                response.try_recv().is_ok(),
                "processed action should have a response"
            );
        }
        for response in responses.iter_mut().skip(2) {
            assert!(
                response.try_recv().is_err(),
                "unprocessed action should remain queued"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn advance_single_tick_issues_direct_worker_step() {
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
        let mut listener =
            LocalListener::bind(&socket_path).expect("fake instance listener should bind");
        let server = tokio::spawn(async move {
            loop {
                let mut stream = listener
                    .accept()
                    .await
                    .expect("fake instance should accept worker-step request");
                let mut line = String::new();
                let mut reader = BufReader::new(&mut stream);
                reader
                    .read_line(&mut line)
                    .await
                    .expect("request should be readable");
                if line.is_empty() {
                    continue;
                }
                drop(reader);
                let request: Request =
                    serde_json::from_str(line.trim_end()).expect("request json should parse");
                assert!(matches!(
                    request.action,
                    RequestAction::Worker(WorkerAction::Step)
                ));
                let response = Response::ok(request.id, ResponseData::Ack);
                let mut payload =
                    serde_json::to_string(&response).expect("response should serialize");
                payload.push('\n');
                stream
                    .write_all(payload.as_bytes())
                    .await
                    .expect("response should be writable");
                break;
            }
        });
        let worker = instance_worker::InstanceWorker::connect(instance)
            .await
            .expect("test worker should connect to fake instance");

        let mut state = EnvState {
            name: "env".to_string(),
            socket_path: PathBuf::new(),
            tick_duration_us: 20,
            instances: vec![instance.to_string()],
            signal_catalog: EnvSignalCatalog::default(),
            instance_workers: HashMap::from([(instance.to_string(), worker)]),
            time: TimeEngine::default(),
            realtime_tick_backlog: 0,
            can_buses: BTreeMap::new(),
            trace: EnvTraceState {
                active: None,
                last_path: None,
                last_row_count: 0,
                last_signal_count: 0,
                last_period_us: None,
            },
            shutdown: false,
        };

        tick::advance_single_tick(&mut state)
            .await
            .expect("worker step should succeed");
        server.await.expect("fake instance task should finish");

        assert_eq!(state.time.status(state.tick_duration_us).elapsed_ticks, 1);

        restore_agent_sim_home(original_home);
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn instance_worker_times_out_when_instance_never_replies() {
        let home = tempfile::tempdir().expect("temp AGENT_SIM_HOME should be creatable");
        let original_home = std::env::var_os("AGENT_SIM_HOME");
        unsafe {
            std::env::set_var("AGENT_SIM_HOME", home.path());
        }

        let instance = "instance-timeout";
        let socket_path = crate::daemon::lifecycle::socket_path(instance);
        std::fs::create_dir_all(
            socket_path
                .parent()
                .expect("instance socket should have a parent directory"),
        )
        .expect("instance socket parent should be creatable");
        let mut listener =
            LocalListener::bind(&socket_path).expect("fake instance listener should bind");
        let server = tokio::spawn(async move {
            let mut stream = listener
                .accept()
                .await
                .expect("fake instance should accept worker connection");
            let mut line = String::new();
            let mut reader = BufReader::new(&mut stream);
            reader
                .read_line(&mut line)
                .await
                .expect("request should be readable");
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let worker = instance_worker::InstanceWorker::connect_with_timeout(
            instance,
            Duration::from_millis(100),
        )
        .await
        .expect("test worker should connect to fake instance");
        let response_rx = worker
            .begin_instance_request(InstanceAction::Info)
            .await
            .expect("info request should queue");
        let response = response_rx.await.expect("timeout response should arrive");
        let err = response.expect_err("worker request should time out");
        assert!(
            err.contains("timed out reading env worker response"),
            "unexpected timeout error: {err}"
        );

        let follow_up = worker.begin_worker_request(WorkerAction::CanBuses).await;
        assert!(
            follow_up.is_err(),
            "worker connection should close after timeout"
        );

        server.await.expect("fake instance task should finish");
        restore_agent_sim_home(original_home);
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn advance_single_tick_errors_without_advancing_time_when_worker_times_out() {
        let home = tempfile::tempdir().expect("temp AGENT_SIM_HOME should be creatable");
        let original_home = std::env::var_os("AGENT_SIM_HOME");
        unsafe {
            std::env::set_var("AGENT_SIM_HOME", home.path());
        }

        let instance = "instance-timeout";
        let socket_path = crate::daemon::lifecycle::socket_path(instance);
        std::fs::create_dir_all(
            socket_path
                .parent()
                .expect("instance socket should have a parent directory"),
        )
        .expect("instance socket parent should be creatable");
        let mut listener =
            LocalListener::bind(&socket_path).expect("fake instance listener should bind");
        let server = tokio::spawn(async move {
            let mut stream = listener
                .accept()
                .await
                .expect("fake instance should accept worker connection");
            let mut line = String::new();
            let mut reader = BufReader::new(&mut stream);
            reader
                .read_line(&mut line)
                .await
                .expect("request should be readable");
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let worker = instance_worker::InstanceWorker::connect_with_timeout(
            instance,
            Duration::from_millis(100),
        )
        .await
        .expect("test worker should connect to fake instance");
        let mut state = EnvState {
            name: "env".to_string(),
            socket_path: PathBuf::new(),
            tick_duration_us: 20,
            instances: vec![instance.to_string()],
            signal_catalog: EnvSignalCatalog::default(),
            instance_workers: HashMap::from([(instance.to_string(), worker)]),
            time: TimeEngine::default(),
            realtime_tick_backlog: 0,
            can_buses: BTreeMap::new(),
            trace: EnvTraceState {
                active: None,
                last_path: None,
                last_row_count: 0,
                last_signal_count: 0,
                last_period_us: None,
            },
            shutdown: false,
        };

        let err = tick::advance_single_tick(&mut state)
            .await
            .expect_err("worker timeout should fail the env tick");
        assert!(
            err.contains("timed out reading env worker response"),
            "unexpected timeout error: {err}"
        );
        assert_eq!(state.time.status(state.tick_duration_us).elapsed_ticks, 0);

        server.await.expect("fake instance task should finish");
        restore_agent_sim_home(original_home);
    }

    #[test]
    fn duration_to_env_ticks_rejects_zero_tick_duration() {
        let err = duration_to_env_ticks(0, "10ms").expect_err("zero tick duration must fail");
        assert!(err.contains("must be greater than zero"));
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial]
    async fn instance_worker_supports_multiple_requests_on_one_connection() {
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
        let mut listener =
            LocalListener::bind(&socket_path).expect("fake instance listener should bind");
        let server = tokio::spawn(async move {
            let stream = listener
                .accept()
                .await
                .expect("fake instance should accept worker connection");
            let (read_half, mut write_half) = split(stream);
            let mut reader = BufReader::new(read_half);
            for expected_action in [
                RequestAction::Instance(InstanceAction::Info),
                RequestAction::Worker(WorkerAction::CanBuses),
            ] {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .await
                    .expect("request should be readable");
                let request: Request =
                    serde_json::from_str(line.trim_end()).expect("request json should parse");
                match (&request.action, &expected_action) {
                    (
                        RequestAction::Instance(InstanceAction::Info),
                        RequestAction::Instance(InstanceAction::Info),
                    ) => {
                        let response = Response::ok(
                            request.id,
                            ResponseData::ProjectInfo {
                                libpath: "demo.dll".to_string(),
                                tick_duration_us: 20,
                                signal_count: 3,
                            },
                        );
                        let mut payload =
                            serde_json::to_string(&response).expect("response should serialize");
                        payload.push('\n');
                        write_half
                            .write_all(payload.as_bytes())
                            .await
                            .expect("response should be writable");
                    }
                    (
                        RequestAction::Worker(WorkerAction::CanBuses),
                        RequestAction::Worker(WorkerAction::CanBuses),
                    ) => {
                        let response = Response::ok(
                            request.id,
                            ResponseData::CanBuses {
                                buses: vec![crate::protocol::CanBusData {
                                    id: 1,
                                    name: "internal".to_string(),
                                    bitrate: 500_000,
                                    bitrate_data: 0,
                                    fd_capable: false,
                                    attached_iface: None,
                                }],
                            },
                        );
                        let mut payload =
                            serde_json::to_string(&response).expect("response should serialize");
                        payload.push('\n');
                        write_half
                            .write_all(payload.as_bytes())
                            .await
                            .expect("response should be writable");
                    }
                    other => panic!("unexpected request sequence: {other:?}"),
                }
            }
        });

        let worker = instance_worker::InstanceWorker::connect(instance)
            .await
            .expect("test worker should connect to fake instance");

        let info_rx = worker
            .begin_instance_request(InstanceAction::Info)
            .await
            .expect("info request should queue");
        let info = info_rx.await.expect("info response should arrive");
        assert!(matches!(
            info,
            Ok(ResponseData::ProjectInfo {
                tick_duration_us: 20,
                signal_count: 3,
                ..
            })
        ));

        let buses_rx = worker
            .begin_worker_request(WorkerAction::CanBuses)
            .await
            .expect("can buses request should queue");
        let buses = buses_rx.await.expect("can buses response should arrive");
        assert!(matches!(
            buses,
            Ok(ResponseData::CanBuses { buses }) if buses.len() == 1 && buses[0].name == "internal"
        ));

        server.await.expect("fake instance task should finish");
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
        let err = bootstrap::bootstrap_instance_detached_with_exe(&instance, &missing_exe)
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
