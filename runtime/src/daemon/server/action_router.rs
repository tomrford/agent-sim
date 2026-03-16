use super::DaemonState;
use super::{can_ops, shared_ops};
use crate::can::CanSocket;
use crate::can::dbc::DbcBusOverlay;
use crate::protocol::{
    CanBusData, InstanceAction, InstanceInfoData, ResponseData, SharedChannelData,
    SharedSlotValueData, SignalData, SignalValueData, WorkerAction, WorkerSignalValueData,
    parse_duration_us,
};
use crate::shared::SharedRegion;
use crate::signal_selectors;
use crate::sim::error::SimError;
use std::path::Path;

pub(super) async fn dispatch_instance_action(
    action: InstanceAction,
    state: &mut DaemonState,
) -> Result<ResponseData, String> {
    match action {
        InstanceAction::Ping => Ok(ResponseData::Ack),
        InstanceAction::Load { load_spec } => {
            let bound = state.project.libpath.display().to_string();
            if load_spec.libpath != bound {
                return Err(format!(
                    "daemon already bound to '{bound}'; start a new instance for a different DLL"
                ));
            }
            Ok(ResponseData::Loaded {
                libpath: bound,
                signal_count: state.project.signals().len(),
            })
        }
        InstanceAction::Info => Ok(ResponseData::ProjectInfo {
            libpath: state.project.libpath.display().to_string(),
            tick_duration_us: state.project.tick_duration_us(),
            signal_count: state.project.signals().len(),
        }),
        InstanceAction::Signals => {
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
        InstanceAction::Reset => {
            state.project.reset().map_err(|e| e.to_string())?;
            state.time.reset();
            state.realtime_tick_backlog = 0;
            Ok(ResponseData::Ack)
        }
        InstanceAction::Get { selectors } => {
            let values = read_selected_signal_values(state, selectors)?;
            Ok(ResponseData::SignalValues { values })
        }
        InstanceAction::Sample { selectors } => {
            let values = read_selected_signal_values(state, selectors)?;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::SignalSample {
                tick: status.elapsed_ticks,
                time_us: status.elapsed_time_us,
                values,
            })
        }
        InstanceAction::Set { writes } => {
            let mut applied = 0_usize;
            for (selector, raw_value) in writes {
                if selector.starts_with("can.") {
                    return Err(can_signal_projection_error());
                }

                let ids = signal_selectors::select_instance_signal_ids(
                    &state.project,
                    std::slice::from_ref(&selector),
                )
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
        InstanceAction::TraceStart { path, period } => {
            start_instance_trace(state, &path, &period)?;
            Ok(trace_status_response(state))
        }
        InstanceAction::TraceStop => {
            stop_instance_trace(state);
            Ok(trace_status_response(state))
        }
        InstanceAction::TraceClear => {
            clear_instance_trace(state)?;
            Ok(trace_status_response(state))
        }
        InstanceAction::TraceStatus => Ok(trace_status_response(state)),
        InstanceAction::TimeStart => {
            reject_local_time_control(state)?;
            state.time.start().map_err(|e| e.to_string())?;
            state.realtime_tick_backlog = 0;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        InstanceAction::TimePause => {
            reject_local_time_control(state)?;
            state.time.pause().map_err(|e| e.to_string())?;
            state.realtime_tick_backlog = 0;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        InstanceAction::TimeStep { duration } => {
            reject_local_time_control(state)?;
            let duration_us = parse_duration_us(&duration).map_err(|e| e.to_string())?;
            state.realtime_tick_backlog = 0;
            let step = state
                .time
                .step_ticks(state.project.tick_duration_us(), duration_us)
                .map_err(|e| e.to_string())?;
            advance_project_ticks_for_request(state, step.advanced_ticks)?;
            Ok(ResponseData::TimeAdvanced {
                requested_us: step.requested_us,
                advanced_ticks: step.advanced_ticks,
                advanced_us: step.advanced_us,
            })
        }
        InstanceAction::TimeSpeed { multiplier } => {
            reject_local_time_control(state)?;
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
        InstanceAction::TimeStatus => {
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        InstanceAction::CanBuses => {
            reject_local_can_control(state)?;
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
        InstanceAction::CanAttach {
            bus_name,
            vcan_iface,
        } => {
            reject_local_can_control(state)?;
            attach_can_bus(state, &bus_name, &vcan_iface)?;
            Ok(ResponseData::Ack)
        }
        InstanceAction::CanDetach { bus_name } => {
            reject_local_can_control(state)?;
            if state.can_attached.remove(&bus_name).is_none() {
                return Err(format!("CAN bus '{bus_name}' is not attached"));
            }
            state.dbc_overlays.remove(&bus_name);
            Ok(ResponseData::Ack)
        }
        InstanceAction::CanLoadDbc { bus_name, path } => {
            reject_local_can_control(state)?;
            let _ = can_ops::find_can_bus_meta(state, &bus_name)?;
            let overlay = DbcBusOverlay::load(Path::new(&path))?;
            let signal_count = overlay.signal_names().count();
            state.dbc_overlays.insert(bus_name.clone(), overlay);
            Ok(ResponseData::DbcLoaded {
                bus: bus_name,
                signal_count,
            })
        }
        InstanceAction::SharedList => {
            let buses = state
                .project
                .shared_channels()
                .iter()
                .map(|channel| SharedChannelData {
                    id: channel.id,
                    name: channel.name.clone(),
                    slot_count: channel.slot_count,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::SharedChannels { channels: buses })
        }
        InstanceAction::SharedAttach {
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
            let meta = shared_ops::find_shared_channel_meta(state, &channel_name)?;
            let mut region = SharedRegion::open(
                Path::new(&path),
                meta.slot_count as usize,
                &writer_session,
                writer,
            )?;
            if writer {
                let snapshot = state.project.shared_write(meta.id).map_err(|e| {
                    format!("sim_shared_write failed for channel '{channel_name}': {e}")
                })?;
                region.publish(&snapshot).map_err(|e| {
                    format!("failed priming shared channel '{channel_name}' snapshot: {e}")
                })?;
            }
            state.shared_attached.insert(
                channel_name.clone(),
                super::AttachedSharedChannel {
                    meta,
                    region,
                    writer,
                },
            );
            Ok(ResponseData::Ack)
        }
        InstanceAction::SharedGet { channel_name } => {
            let attachment = state
                .shared_attached
                .get(&channel_name)
                .ok_or_else(|| format!("shared channel '{channel_name}' is not attached"))?;
            let slots = attachment
                .region
                .read_snapshot()
                .map_err(|e| format!("failed reading shared channel '{channel_name}': {e}"))?
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
        InstanceAction::CanSend {
            bus_name,
            arb_id,
            data_hex,
            flags,
        } => {
            reject_local_can_control(state)?;
            let attachment = state
                .can_attached
                .get(&bus_name)
                .ok_or_else(|| format!("CAN bus '{bus_name}' is not attached"))?;
            let payload = crate::can::parse_data_hex(&data_hex)?;
            let mut data = [0_u8; 64];
            data[..payload.len()].copy_from_slice(&payload);
            let frame = crate::sim::types::SimCanFrame {
                arb_id,
                len: payload.len() as u8,
                flags: flags.unwrap_or(0),
                data,
            };
            crate::can::validate_frame(&attachment.meta.name, attachment.meta.fd_capable, &frame)?;
            attachment.socket.send(&frame)?;
            can_ops::record_frame(state, &bus_name, &frame);
            Ok(ResponseData::CanSend {
                bus: bus_name,
                arb_id,
                len: frame.len,
            })
        }
        InstanceAction::InstanceStatus => Ok(ResponseData::InstanceStatus {
            instance: state.session.clone(),
            socket_path: state.socket_path.display().to_string(),
            running: true,
            env: state.env.clone(),
        }),
        InstanceAction::InstanceList => {
            let instances = crate::daemon::lifecycle::list_instances()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|instance| InstanceInfoData {
                    name: instance.name,
                    socket_path: instance.socket_path.display().to_string(),
                    running: instance.running,
                    env: instance.env,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::InstanceList { instances })
        }
        InstanceAction::Close => {
            state.shutdown = true;
            Ok(ResponseData::Ack)
        }
    }
}

pub(super) async fn dispatch_worker_action(
    action: WorkerAction,
    state: &mut DaemonState,
) -> Result<ResponseData, String> {
    match action {
        WorkerAction::CanBuses => {
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
                    attached_iface: None,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::CanBuses { buses })
        }
        WorkerAction::CanAttach {
            bus_name,
            vcan_iface,
        } => {
            attach_can_bus(state, &bus_name, &vcan_iface)?;
            Ok(ResponseData::Ack)
        }
        WorkerAction::ReadSignals { ids } => {
            let values = read_signal_values_by_ids(state, &ids)?;
            Ok(ResponseData::WorkerSignalValues { values })
        }
        WorkerAction::CanDiscardPendingRx => {
            can_ops::discard_can_rx(state)?;
            Ok(ResponseData::Ack)
        }
        WorkerAction::Step => {
            advance_single_project_tick(state).map_err(TickStepError::into_message)?;
            state.time.advance_ticks(1);
            Ok(ResponseData::Ack)
        }
    }
}

fn read_signal_values_by_ids(
    state: &DaemonState,
    ids: &[u32],
) -> Result<Vec<WorkerSignalValueData>, String> {
    let signal_values = state.project.read_many(ids).map_err(|e| e.to_string())?;
    if signal_values.len() != ids.len() {
        return Err(format!(
            "grouped signal read returned {} values for {} requested ids",
            signal_values.len(),
            ids.len()
        ));
    }

    let mut values = Vec::with_capacity(signal_values.len());
    for (id, value) in ids.iter().copied().zip(signal_values) {
        let signal = state
            .project
            .signal_by_id(id)
            .ok_or_else(|| SimError::InvalidSignal(format!("#{id}")).to_string())?;
        if signal.signal_type != value.signal_type() {
            return Err(SimError::TypeMismatch {
                name: signal.name.clone(),
                expected: signal.signal_type,
                actual: value.signal_type(),
            }
            .to_string());
        }
        values.push(WorkerSignalValueData { id, value });
    }
    Ok(values)
}

fn read_selected_signal_values(
    state: &DaemonState,
    selectors: Vec<String>,
) -> Result<Vec<SignalValueData>, String> {
    let mut values = Vec::new();

    for selector in selectors {
        if selector.starts_with("can.") {
            return Err(can_signal_projection_error());
        }
        let ids = signal_selectors::select_instance_signal_ids(
            &state.project,
            std::slice::from_ref(&selector),
        )
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

    Ok(values)
}

fn advance_project_ticks_for_request(state: &mut DaemonState, ticks: u64) -> Result<(), String> {
    for _ in 0..ticks {
        if let Err(err) = advance_single_project_tick(state) {
            if err.advance_tick() {
                state.time.advance_ticks(1);
            }
            return Err(err.into_message());
        }
        state.time.advance_ticks(1);
    }
    Ok(())
}

pub(super) fn advance_single_project_tick(state: &mut DaemonState) -> Result<(), TickStepError> {
    can_ops::process_can_rx(state).map_err(TickStepError::pre_tick)?;
    shared_ops::process_shared_rx(state).map_err(TickStepError::pre_tick)?;
    state
        .project
        .tick()
        .map_err(|e| TickStepError::pre_tick(e.to_string()))?;
    sample_instance_trace_after_tick(state).map_err(TickStepError::post_tick)?;
    can_ops::process_can_tx(state).map_err(TickStepError::post_tick)?;
    shared_ops::process_shared_tx(state).map_err(TickStepError::post_tick)?;
    Ok(())
}

fn attach_can_bus(state: &mut DaemonState, bus_name: &str, vcan_iface: &str) -> Result<(), String> {
    if state.can_attached.contains_key(bus_name) {
        return Err(format!("CAN bus '{bus_name}' is already attached"));
    }
    let meta = can_ops::find_can_bus_meta(state, bus_name)?;
    let socket = CanSocket::open(vcan_iface, meta.bitrate, meta.bitrate_data, meta.fd_capable)?;
    state
        .can_attached
        .insert(bus_name.to_string(), super::AttachedCanBus { meta, socket });
    Ok(())
}

pub(super) struct TickStepError {
    message: String,
    advance_tick: bool,
}

impl TickStepError {
    fn pre_tick(message: String) -> Self {
        Self {
            message,
            advance_tick: false,
        }
    }

    fn post_tick(message: String) -> Self {
        Self {
            message,
            advance_tick: true,
        }
    }

    pub(super) fn advance_tick(&self) -> bool {
        self.advance_tick
    }

    pub(super) fn into_message(self) -> String {
        self.message
    }
}

fn reject_local_time_control(state: &DaemonState) -> Result<(), String> {
    if let Some(env_name) = &state.env {
        return Err(format!(
            "instance-local time control is unavailable while attached to env '{env_name}'; use `agent-sim env time {env_name} ...` instead"
        ));
    }
    Ok(())
}

fn reject_local_can_control(state: &DaemonState) -> Result<(), String> {
    if state.env.is_some() {
        Err(local_can_control_error())
    } else {
        Ok(())
    }
}

fn local_can_control_error() -> String {
    "CAN is env-owned; use `agent-sim env can <env> ...` instead".to_string()
}

fn can_signal_projection_error() -> String {
    "CAN signal projection is no longer supported; use `agent-sim env can <env> ...` instead"
        .to_string()
}

fn start_instance_trace(state: &mut DaemonState, path: &str, period: &str) -> Result<(), String> {
    if state.trace.active.is_some() {
        return Err("trace is already active; stop or clear it first".to_string());
    }
    if !Path::new(path).is_absolute() {
        return Err(format!(
            "trace output path must be absolute, got '{}'",
            path
        ));
    }
    let period_us = parse_duration_us(period).map_err(|err| err.to_string())?;
    if period_us == 0 {
        return Err("trace period must be greater than zero".to_string());
    }

    let signal_ids = state
        .project
        .signals()
        .iter()
        .map(|signal| signal.id)
        .collect::<Vec<_>>();
    let headers = state
        .project
        .signals()
        .iter()
        .map(|signal| signal.name.clone())
        .collect::<Vec<_>>();
    let period_ticks = period_us.div_ceil(u64::from(state.project.tick_duration_us()));

    let status = state.time.status(state.project.tick_duration_us());
    let mut writer = crate::trace::CsvTraceWriter::create(path, &headers)?;
    let values = state
        .project
        .read_many(&signal_ids)
        .map_err(|e| e.to_string())?;
    writer.write_row(status.elapsed_ticks, status.elapsed_time_us, &values)?;

    state.trace.active = Some(super::ActiveDaemonTrace {
        writer,
        signal_ids,
        period_ticks,
        period_us,
        next_due_tick: status.elapsed_ticks.saturating_add(period_ticks),
    });
    update_trace_history_from_active(state);
    Ok(())
}

fn stop_instance_trace(state: &mut DaemonState) {
    if let Some(active) = state.trace.active.take() {
        state.trace.last_path = Some(active.writer.path().to_path_buf());
        state.trace.last_row_count = active.writer.row_count();
        state.trace.last_signal_count = active.signal_ids.len();
        state.trace.last_period_us = Some(active.period_us);
    }
}

fn clear_instance_trace(state: &mut DaemonState) -> Result<(), String> {
    let path = if let Some(active) = state.trace.active.take() {
        state.trace.last_row_count = active.writer.row_count();
        Some(active.writer.path().to_path_buf())
    } else {
        state.trace.last_path.clone()
    };
    if let Some(path) = path
        && path.exists()
    {
        std::fs::remove_file(&path)
            .map_err(|err| format!("failed to remove trace file '{}': {err}", path.display()))?;
    }
    state.trace.last_path = None;
    state.trace.last_row_count = 0;
    state.trace.last_signal_count = 0;
    state.trace.last_period_us = None;
    Ok(())
}

fn trace_status_response(state: &DaemonState) -> ResponseData {
    if let Some(active) = &state.trace.active {
        ResponseData::TraceStatus {
            active: true,
            path: Some(active.writer.path().display().to_string()),
            row_count: active.writer.row_count(),
            signal_count: active.signal_ids.len(),
            period_us: Some(active.period_us),
        }
    } else {
        ResponseData::TraceStatus {
            active: false,
            path: state
                .trace
                .last_path
                .as_ref()
                .map(|path| path.display().to_string()),
            row_count: state.trace.last_row_count,
            signal_count: state.trace.last_signal_count,
            period_us: state.trace.last_period_us,
        }
    }
}

fn sample_instance_trace_after_tick(state: &mut DaemonState) -> Result<(), String> {
    let Some(active) = &mut state.trace.active else {
        return Ok(());
    };
    let next_tick = state
        .time
        .status(state.project.tick_duration_us())
        .elapsed_ticks
        .saturating_add(1);
    if next_tick < active.next_due_tick {
        return Ok(());
    }

    let time_us = next_tick.saturating_mul(u64::from(state.project.tick_duration_us()));
    let values = state
        .project
        .read_many(&active.signal_ids)
        .map_err(|e| e.to_string())?;
    active.writer.write_row(next_tick, time_us, &values)?;
    active.next_due_tick = next_tick.saturating_add(active.period_ticks);
    update_trace_history_from_active(state);
    Ok(())
}

fn update_trace_history_from_active(state: &mut DaemonState) {
    if let Some(active) = &state.trace.active {
        state.trace.last_path = Some(active.writer.path().to_path_buf());
        state.trace.last_row_count = active.writer.row_count();
        state.trace.last_signal_count = active.signal_ids.len();
        state.trace.last_period_us = Some(active.period_us);
    }
}
