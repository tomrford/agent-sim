use super::{
    CanScheduleJob, EnvState, duration_to_env_ticks, frame_data, locate_schedule_bus,
    locate_schedule_mut, parse_env_frame, reset_env_can_state, send_env_frame, start_schedule,
    update_schedule,
};
use crate::can::dbc::DbcBusOverlay;
use crate::protocol::{
    CanBusData, CanScheduleData, EnvAction, EnvSignalData, EnvSignalValueData, InstanceAction,
    ResponseData, WorkerAction, WorkerSignalValueData,
};
use std::collections::BTreeMap;

pub(super) async fn dispatch_action(
    action: EnvAction,
    state: &mut EnvState,
) -> Result<ResponseData, String> {
    match action {
        EnvAction::Status { env } => {
            ensure_env_name(state, &env)?;
            Ok(ResponseData::EnvStatus {
                env,
                running: true,
                instance_count: state.instances.len(),
                tick_duration_us: state.tick_duration_us,
            })
        }
        EnvAction::Reset { env } => {
            ensure_env_name(state, &env)?;
            let mut pending = Vec::with_capacity(state.instances.len());
            for instance in &state.instances {
                let worker = state
                    .instance_workers
                    .get(instance)
                    .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
                let response_rx = worker.begin_instance_request(InstanceAction::Reset).await?;
                pending.push((instance.clone(), response_rx));
            }
            for (instance, response_rx) in pending {
                let response = response_rx.await.map_err(|_| {
                    format!("reset response channel closed for instance '{instance}'")
                })??;
                if !matches!(response, ResponseData::Ack) {
                    return Err(format!(
                        "unexpected reset payload while resetting instance '{instance}'"
                    ));
                }
            }
            let mut pending_discard = Vec::with_capacity(state.instances.len());
            for instance in &state.instances {
                let worker = state
                    .instance_workers
                    .get(instance)
                    .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
                let response_rx = worker
                    .begin_worker_request(WorkerAction::CanDiscardPendingRx)
                    .await?;
                pending_discard.push((instance.clone(), response_rx));
            }
            for (instance, response_rx) in pending_discard {
                let response = response_rx.await.map_err(|_| {
                    format!("CAN discard response channel closed for instance '{instance}'")
                })??;
                if !matches!(response, ResponseData::Ack) {
                    return Err(format!(
                        "unexpected CAN discard payload while resetting instance '{instance}'"
                    ));
                }
            }
            reset_env_can_state(state);
            state.time.reset();
            state.realtime_tick_backlog = 0;
            Ok(ResponseData::Ack)
        }
        EnvAction::TimeStart { env } => {
            ensure_env_name(state, &env)?;
            state.time.start().map_err(|err| err.to_string())?;
            state.realtime_tick_backlog = 0;
            env_time_status(state)
        }
        EnvAction::TimePause { env } => {
            ensure_env_name(state, &env)?;
            state.time.pause().map_err(|err| err.to_string())?;
            state.realtime_tick_backlog = 0;
            env_time_status(state)
        }
        EnvAction::TimeSpeed { env, multiplier } => {
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
        EnvAction::TimeStatus { env } => {
            ensure_env_name(state, &env)?;
            env_time_status(state)
        }
        EnvAction::CanBuses { env } => {
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
        EnvAction::CanLoadDbc {
            env,
            bus_name,
            path,
        } => {
            ensure_env_name(state, &env)?;
            let bus = state
                .can_buses
                .get_mut(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            let overlay = DbcBusOverlay::load(std::path::Path::new(&path))?;
            let signal_count = overlay.signal_names().count();
            bus.dbc = Some(overlay);
            Ok(ResponseData::DbcLoaded {
                bus: bus_name,
                signal_count,
            })
        }
        EnvAction::CanSend {
            env,
            bus_name,
            arb_id,
            data_hex,
            flags,
        } => {
            ensure_env_name(state, &env)?;
            let frame = parse_env_frame(state, &bus_name, arb_id, &data_hex, flags.unwrap_or(0))?;
            send_env_frame(state, &bus_name, &frame)?;
            Ok(ResponseData::CanSend {
                bus: bus_name,
                arb_id,
                len: frame.len,
            })
        }
        EnvAction::CanInspect { env, bus_name } => {
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
        EnvAction::CanScheduleAdd {
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
            super::ensure_unique_schedule_job_id(
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
        EnvAction::CanScheduleUpdate {
            env,
            job_id,
            arb_id,
            data_hex,
            every,
            flags,
        } => {
            ensure_env_name(state, &env)?;
            let every_ticks = duration_to_env_ticks(state.tick_duration_us, &every)?;
            let current_tick = state.time.status(state.tick_duration_us).elapsed_ticks;
            let bus_name = locate_schedule_bus(state, &job_id)?;
            let frame = parse_env_frame(state, &bus_name, arb_id, &data_hex, flags.unwrap_or(0))?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            update_schedule(schedule, arb_id, data_hex, frame, every_ticks, current_tick);
            Ok(ResponseData::Ack)
        }
        EnvAction::CanScheduleRemove { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (bus_name, _) = locate_schedule_mut(state, &job_id)?;
            let bus = state
                .can_buses
                .get_mut(&bus_name)
                .ok_or_else(|| format!("env CAN bus '{bus_name}' not found"))?;
            bus.schedules.remove(&job_id);
            Ok(ResponseData::Ack)
        }
        EnvAction::CanScheduleStop { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            schedule.enabled = false;
            Ok(ResponseData::Ack)
        }
        EnvAction::CanScheduleStart { env, job_id } => {
            ensure_env_name(state, &env)?;
            let (_, schedule) = locate_schedule_mut(state, &job_id)?;
            start_schedule(schedule);
            Ok(ResponseData::Ack)
        }
        EnvAction::CanScheduleList { env, bus_name } => {
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
        EnvAction::TraceStart { env, path, period } => {
            ensure_env_name(state, &env)?;
            start_env_trace(state, &path, &period).await?;
            Ok(trace_status_response(state))
        }
        EnvAction::TraceStop { env } => {
            ensure_env_name(state, &env)?;
            stop_env_trace(state);
            Ok(trace_status_response(state))
        }
        EnvAction::TraceClear { env } => {
            ensure_env_name(state, &env)?;
            clear_env_trace(state)?;
            Ok(trace_status_response(state))
        }
        EnvAction::TraceStatus { env } => {
            ensure_env_name(state, &env)?;
            Ok(trace_status_response(state))
        }
        EnvAction::Signals { env, selectors } => {
            ensure_env_name(state, &env)?;
            let resolved = state.signal_catalog.resolve_selectors(&selectors)?;
            let signals = resolved
                .into_iter()
                .map(|index| {
                    let entry = &state.signal_catalog.entries()[index];
                    EnvSignalData {
                        instance: entry.instance.clone(),
                        local_id: entry.local_id,
                        name: entry.qualified_name.clone(),
                        signal_type: entry.signal_type,
                        units: entry.units.clone(),
                    }
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::EnvSignals { signals })
        }
        EnvAction::Get { env, selectors } => {
            ensure_env_name(state, &env)?;
            let values = read_env_signal_values(state, &selectors).await?;
            Ok(ResponseData::EnvSignalValues { values })
        }
        EnvAction::Close { env } => {
            ensure_env_name(state, &env)?;
            state.shutdown = true;
            Ok(ResponseData::Ack)
        }
        EnvAction::TimeStep { env, duration } => {
            ensure_env_name(state, &env)?;
            let duration_us =
                crate::protocol::parse_duration_us(&duration).map_err(|err| err.to_string())?;
            state.realtime_tick_backlog = 0;
            let step = state
                .time
                .step_ticks(state.tick_duration_us, duration_us)
                .map_err(|err| err.to_string())?;
            crate::envd::server::tick::advance_due_ticks(state, step.advanced_ticks).await?;
            Ok(ResponseData::TimeAdvanced {
                requested_us: step.requested_us,
                advanced_ticks: step.advanced_ticks,
                advanced_us: step.advanced_us,
            })
        }
    }
}

pub(super) fn ensure_env_name(state: &EnvState, requested: &str) -> Result<(), String> {
    if state.name == requested {
        Ok(())
    } else {
        Err(format!(
            "env daemon '{}' cannot service requests for env '{}'",
            state.name, requested
        ))
    }
}

pub(super) fn env_time_status(state: &EnvState) -> Result<ResponseData, String> {
    let status = state.time.status(state.tick_duration_us);
    Ok(ResponseData::TimeStatus {
        state: status.state,
        elapsed_ticks: status.elapsed_ticks,
        elapsed_time_us: status.elapsed_time_us,
        speed: status.speed,
    })
}

async fn read_env_signal_values(
    state: &EnvState,
    selectors: &[String],
) -> Result<Vec<EnvSignalValueData>, String> {
    let resolved = state.signal_catalog.resolve_selectors(selectors)?;
    let mut grouped = BTreeMap::<String, Vec<(usize, u32)>>::new();
    for (output_index, catalog_index) in resolved.iter().enumerate() {
        let entry = &state.signal_catalog.entries()[*catalog_index];
        grouped
            .entry(entry.instance.clone())
            .or_default()
            .push((output_index, entry.local_id));
    }

    let mut values = vec![None; resolved.len()];
    for (instance, request_items) in grouped {
        let worker = state
            .instance_workers
            .get(&instance)
            .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
        let ids = request_items
            .iter()
            .map(|(_, signal_id)| *signal_id)
            .collect::<Vec<_>>();
        let response_rx = worker
            .begin_worker_request(WorkerAction::ReadSignals { ids: ids.clone() })
            .await?;
        let response = response_rx.await.map_err(|_| {
            format!("read-signals response channel closed for instance '{instance}'")
        })??;
        let ResponseData::WorkerSignalValues {
            values: worker_values,
        } = response
        else {
            return Err(format!(
                "unexpected read-signals payload while reading instance '{instance}'"
            ));
        };
        validate_worker_signal_values(&instance, &ids, &worker_values)?;
        for ((output_index, _), worker_value) in request_items.into_iter().zip(worker_values) {
            let catalog_index = resolved[output_index];
            let entry = &state.signal_catalog.entries()[catalog_index];
            values[output_index] = Some(EnvSignalValueData {
                instance: entry.instance.clone(),
                local_id: entry.local_id,
                name: entry.qualified_name.clone(),
                signal_type: entry.signal_type,
                value: worker_value.value,
                units: entry.units.clone(),
            });
        }
    }

    values
        .into_iter()
        .map(|value| value.ok_or_else(|| "missing grouped env signal value".to_string()))
        .collect()
}

fn validate_worker_signal_values(
    instance: &str,
    requested_ids: &[u32],
    values: &[WorkerSignalValueData],
) -> Result<(), String> {
    if values.len() != requested_ids.len() {
        return Err(format!(
            "instance '{instance}' returned {} values for {} requested ids",
            values.len(),
            requested_ids.len()
        ));
    }
    for (idx, (expected, value)) in requested_ids.iter().zip(values.iter()).enumerate() {
        if *expected != value.id {
            return Err(format!(
                "instance '{instance}' returned mismatched signal id at index {idx}: expected {expected}, got {}",
                value.id
            ));
        }
    }
    Ok(())
}

pub(super) async fn sample_env_trace_after_tick(
    state: &mut EnvState,
    sample_tick: u64,
) -> Result<(), String> {
    let Some(active) = &state.trace.active else {
        return Ok(());
    };
    if sample_tick < active.next_due_tick {
        return Ok(());
    }
    let signals = active.signals.clone();
    let values = read_env_trace_values(state, &signals).await?;
    let time_us = sample_tick.saturating_mul(u64::from(state.tick_duration_us));
    if let Some(active) = &mut state.trace.active {
        active.writer.write_row(sample_tick, time_us, &values)?;
        active.next_due_tick = sample_tick.saturating_add(active.period_ticks);
        update_env_trace_history(state);
    }
    Ok(())
}

async fn start_env_trace(state: &mut EnvState, path: &str, period: &str) -> Result<(), String> {
    if state.trace.active.is_some() {
        return Err("trace is already active; stop or clear it first".to_string());
    }

    let period_us = crate::protocol::parse_duration_us(period).map_err(|err| err.to_string())?;
    if period_us == 0 {
        return Err("trace period must be greater than zero".to_string());
    }
    let period_ticks = period_us.div_ceil(u64::from(state.tick_duration_us));
    let signals = state
        .signal_catalog
        .entries()
        .iter()
        .map(|entry| super::EnvTraceSignal {
            instance: entry.instance.clone(),
            local_id: entry.local_id,
            name: entry.qualified_name.clone(),
        })
        .collect::<Vec<_>>();
    let headers = signals
        .iter()
        .map(|signal| signal.name.clone())
        .collect::<Vec<_>>();

    let status = state.time.status(state.tick_duration_us);
    let mut writer = crate::trace::CsvTraceWriter::create(path, &headers)?;
    let values = read_env_trace_values(state, &signals).await?;
    writer.write_row(status.elapsed_ticks, status.elapsed_time_us, &values)?;

    state.trace.active = Some(super::ActiveEnvTrace {
        writer,
        period_ticks,
        period_us,
        next_due_tick: status.elapsed_ticks.saturating_add(period_ticks),
        signals,
    });
    update_env_trace_history(state);
    Ok(())
}

fn stop_env_trace(state: &mut EnvState) {
    if let Some(active) = state.trace.active.take() {
        state.trace.last_path = Some(active.writer.path().to_path_buf());
        state.trace.last_row_count = active.writer.row_count();
        state.trace.last_signal_count = active.signals.len();
        state.trace.last_period_us = Some(active.period_us);
    }
}

fn clear_env_trace(state: &mut EnvState) -> Result<(), String> {
    let path = if let Some(active) = state.trace.active.take() {
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

fn trace_status_response(state: &EnvState) -> ResponseData {
    if let Some(active) = &state.trace.active {
        ResponseData::TraceStatus {
            active: true,
            path: Some(active.writer.path().display().to_string()),
            row_count: active.writer.row_count(),
            signal_count: active.signals.len(),
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

fn update_env_trace_history(state: &mut EnvState) {
    if let Some(active) = &state.trace.active {
        state.trace.last_path = Some(active.writer.path().to_path_buf());
        state.trace.last_row_count = active.writer.row_count();
        state.trace.last_signal_count = active.signals.len();
        state.trace.last_period_us = Some(active.period_us);
    }
}

async fn read_env_trace_values(
    state: &EnvState,
    signals: &[super::EnvTraceSignal],
) -> Result<Vec<crate::sim::types::SignalValue>, String> {
    let mut grouped = BTreeMap::<String, Vec<(usize, u32)>>::new();
    for (index, signal) in signals.iter().enumerate() {
        grouped
            .entry(signal.instance.clone())
            .or_default()
            .push((index, signal.local_id));
    }

    let mut values = vec![None; signals.len()];
    for (instance, request_items) in grouped {
        let worker = state
            .instance_workers
            .get(&instance)
            .ok_or_else(|| format!("missing env worker for instance '{instance}'"))?;
        let ids = request_items
            .iter()
            .map(|(_, signal_id)| *signal_id)
            .collect::<Vec<_>>();
        let response_rx = worker
            .begin_worker_request(WorkerAction::ReadSignals { ids: ids.clone() })
            .await?;
        let response = response_rx.await.map_err(|_| {
            format!("read-signals response channel closed for instance '{instance}'")
        })??;
        let ResponseData::WorkerSignalValues {
            values: worker_values,
        } = response
        else {
            return Err(format!(
                "unexpected read-signals payload while reading instance '{instance}'"
            ));
        };
        validate_worker_signal_values(&instance, &ids, &worker_values)?;
        for ((output_index, _), worker_value) in request_items.into_iter().zip(worker_values) {
            values[output_index] = Some(worker_value.value);
        }
    }

    values
        .into_iter()
        .map(|value| value.ok_or_else(|| "missing grouped env trace value".to_string()))
        .collect()
}
