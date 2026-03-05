use super::DaemonState;
use super::{can_ops, shared_ops};
use crate::can::CanSocket;
use crate::can::dbc::DbcBusOverlay;
use crate::protocol::{
    Action, CanBusData, ResponseData, SessionInfoData, SharedChannelData, SharedSlotValueData,
    SignalData, SignalValueData, parse_duration_us,
};
use crate::shared::SharedRegion;
use crate::sim::error::SimError;
use crate::sim::types::SimCanFrame;
use std::path::Path;

pub(super) async fn dispatch_action(
    action: Action,
    state: &mut DaemonState,
) -> Result<ResponseData, String> {
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
                    can_ops::write_can_signal(state, &selector, &raw_value)?;
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
            advance_project_ticks_for_request(state, step.advanced_ticks)?;
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
            let meta = can_ops::find_can_bus_meta(state, &bus_name)?;
            let socket = CanSocket::open(&vcan_iface, meta.fd_capable)?;
            state
                .can_attached
                .insert(bus_name.clone(), super::AttachedCanBus { meta, socket });
            Ok(ResponseData::Ack)
        }
        Action::CanDetach { bus_name } => {
            if state.can_attached.remove(&bus_name).is_none() {
                return Err(format!("CAN bus '{bus_name}' is not attached"));
            }
            Ok(ResponseData::Ack)
        }
        Action::CanLoadDbc { bus_name, path } => {
            let _ = can_ops::find_can_bus_meta(state, &bus_name)?;
            can_ops::ensure_absolute_path(&path, "DBC")?;
            let overlay = DbcBusOverlay::load(Path::new(&path))?;
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
            let payload = can_ops::parse_data_hex(&data_hex)?;
            let mut data = [0_u8; 64];
            data[..payload.len()].copy_from_slice(&payload);
            let frame = SimCanFrame {
                arb_id,
                len: payload.len() as u8,
                flags: flags.unwrap_or(0),
                data,
            };
            can_ops::validate_can_frame(&attachment.meta, &frame)?;
            attachment.socket.send(&frame)?;
            can_ops::record_frame(state, &bus_name, &frame);
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

fn read_selected_signal_values(
    state: &DaemonState,
    selectors: Vec<String>,
) -> Result<Vec<SignalValueData>, String> {
    let mut values = Vec::new();
    let mut native_selectors = Vec::new();

    for selector in selectors {
        if selector.starts_with("can.") {
            values.extend(can_ops::get_can_signal_values(state, &selector)?);
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
