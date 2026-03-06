use super::DaemonState;
use super::{can_ops, shared_ops};
use crate::protocol::{
    Action, CanBusData, CanBusFramesData, InstanceInfoData, ResponseData, SharedChannelData,
    SharedSlotValueData, SignalData, SignalValueData, parse_duration_us,
};
use crate::shared::SharedRegion;
use crate::sim::error::SimError;
use std::path::Path;

pub(super) async fn dispatch_action(
    action: Action,
    state: &mut DaemonState,
) -> Result<ResponseData, String> {
    match action {
        Action::Ping => Ok(ResponseData::Ack),
        Action::Load { load_spec } => {
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
            let values = read_selected_signal_values(state, selectors)?;
            Ok(ResponseData::SignalValues { values })
        }
        Action::Sample { selectors } => {
            let values = read_selected_signal_values(state, selectors)?;
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::SignalSample {
                tick: status.elapsed_ticks,
                time_us: status.elapsed_time_us,
                values,
            })
        }
        Action::Set { writes } => {
            let mut applied = 0_usize;
            for (selector, raw_value) in writes {
                if selector.starts_with("can.") {
                    return Err(can_signal_projection_error());
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
            reject_local_time_control(state)?;
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
            reject_local_time_control(state)?;
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
            reject_local_time_control(state)?;
            let duration_us = parse_duration_us(&duration).map_err(|e| e.to_string())?;
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
        Action::TimeSpeed { multiplier } => {
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
        Action::TimeStatus => {
            let status = state.time.status(state.project.tick_duration_us());
            Ok(ResponseData::TimeStatus {
                state: status.state,
                elapsed_ticks: status.elapsed_ticks,
                elapsed_time_us: status.elapsed_time_us,
                speed: status.speed,
            })
        }
        Action::WorkerCanBuses => {
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
        Action::WorkerStep { can_rx } => {
            for batch in &can_rx {
                let meta = can_ops::find_can_bus_meta(state, &batch.bus_name)?;
                let frames = batch
                    .frames
                    .iter()
                    .cloned()
                    .map(crate::sim::types::SimCanFrame::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                for frame in &frames {
                    can_ops::validate_can_frame(&meta, frame)?;
                }
                state
                    .project
                    .can_rx(meta.id, &frames)
                    .map_err(|e| format!("sim_can_rx failed for bus '{}': {e}", batch.bus_name))?;
            }
            shared_ops::process_shared_rx(state)?;
            state.project.tick().map_err(|e| e.to_string())?;
            let mut can_tx = Vec::new();
            for bus in state.project.can_buses() {
                let frames = state
                    .project
                    .can_tx(bus.id)
                    .map_err(|e| format!("sim_can_tx failed for bus '{}': {e}", bus.name))?;
                if !frames.is_empty() {
                    can_tx.push(CanBusFramesData {
                        bus_name: bus.name.clone(),
                        frames: frames.iter().map(Into::into).collect(),
                    });
                }
            }
            shared_ops::process_shared_tx(state)?;
            state.time.advance_ticks(1);
            Ok(ResponseData::WorkerStep { can_tx })
        }
        Action::CanBuses => Err(local_can_control_error()),
        Action::CanAttach { .. } => Err(local_can_control_error()),
        Action::CanDetach { .. } => Err(local_can_control_error()),
        Action::CanLoadDbc { .. } => Err(local_can_control_error()),
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
            let meta = shared_ops::find_shared_channel_meta(state, &channel_name)?;
            let region = SharedRegion::open(
                Path::new(&path),
                meta.slot_count as usize,
                &writer_session,
                writer,
            )?;
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
        Action::SharedGet { channel_name } => {
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
        Action::CanSend { .. } => Err(local_can_control_error()),
        Action::InstanceStatus => Ok(ResponseData::InstanceStatus {
            instance: state.session.clone(),
            socket_path: state.socket_path.display().to_string(),
            running: true,
            env: state.env.clone(),
        }),
        Action::InstanceList => {
            let instances = crate::daemon::lifecycle::list_sessions()
                .await
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|(name, socket_path, running, env)| InstanceInfoData {
                    name,
                    socket_path: socket_path.display().to_string(),
                    running,
                    env,
                })
                .collect::<Vec<_>>();
            Ok(ResponseData::InstanceList { instances })
        }
        Action::Close => {
            state.shutdown = true;
            Ok(ResponseData::Ack)
        }
        Action::EnvStatus { .. }
        | Action::EnvReset { .. }
        | Action::EnvTimeStart { .. }
        | Action::EnvTimePause { .. }
        | Action::EnvTimeStep { .. }
        | Action::EnvTimeSpeed { .. }
        | Action::EnvTimeStatus { .. }
        | Action::EnvCanBuses { .. }
        | Action::EnvCanLoadDbc { .. }
        | Action::EnvCanSend { .. }
        | Action::EnvCanInspect { .. }
        | Action::EnvCanScheduleAdd { .. }
        | Action::EnvCanScheduleUpdate { .. }
        | Action::EnvCanScheduleRemove { .. }
        | Action::EnvCanScheduleStop { .. }
        | Action::EnvCanScheduleStart { .. }
        | Action::EnvCanScheduleList { .. }
        | Action::EnvClose { .. } => Err("env-owned action sent to instance daemon".to_string()),
    }
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
        let ids = DaemonState::select_signal_ids(&state.project, std::slice::from_ref(&selector))
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
    let mut processed = 0_u64;
    for _ in 0..ticks {
        if let Err(err) = can_ops::process_can_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = shared_ops::process_shared_rx(state) {
            state.time.advance_ticks(processed);
            return Err(err);
        }
        if let Err(err) = state.project.tick() {
            state.time.advance_ticks(processed);
            return Err(err.to_string());
        }
        if let Err(err) = can_ops::process_can_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        if let Err(err) = shared_ops::process_shared_tx(state) {
            state.time.advance_ticks(processed.saturating_add(1));
            return Err(err);
        }
        processed = processed.saturating_add(1);
    }
    state.time.advance_ticks(processed);
    Ok(())
}

fn reject_local_time_control(state: &DaemonState) -> Result<(), String> {
    if let Some(env_name) = &state.env {
        return Err(format!(
            "instance-local time control is unavailable while attached to env '{env_name}'; use `agent-sim env time {env_name} ...` instead"
        ));
    }
    Ok(())
}

fn local_can_control_error() -> String {
    "CAN is env-owned; use `agent-sim env can <env> ...` instead".to_string()
}

fn can_signal_projection_error() -> String {
    "CAN signal projection is no longer supported; use `agent-sim env can <env> ...` instead"
        .to_string()
}
