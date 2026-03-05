use super::DaemonState;
use crate::can::dbc::{DbcSignalDef, decode_signal, encode_signal, frame_key_from_frame};
use crate::protocol::SignalValueData;
use crate::sim::types::{
    CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RESERVED_MASK,
    CAN_FLAG_RTR, SignalType, SignalValue, SimCanBusDesc, SimCanFrame,
};
use std::path::Path;

pub(super) fn process_can_rx(state: &mut DaemonState) -> Result<(), String> {
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

pub(super) fn process_can_tx(state: &mut DaemonState) -> Result<(), String> {
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

pub(super) fn find_can_bus_meta(
    state: &DaemonState,
    bus_name: &str,
) -> Result<SimCanBusDesc, String> {
    state
        .project
        .can_buses()
        .iter()
        .find(|bus| bus.name == bus_name)
        .cloned()
        .ok_or_else(|| format!("CAN bus '{bus_name}' not declared by loaded project"))
}

pub(super) fn get_can_signal_values(
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

pub(super) fn write_can_signal(
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
            let mut data = [0_u8; 64];
            let len = signal.message_size.min(64);
            if len == 0 {
                data[0] = 0;
            }
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
    signal: &DbcSignalDef,
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

pub(super) fn record_frame(state: &mut DaemonState, bus_name: &str, frame: &SimCanFrame) {
    let bus_frames = state.frame_state.entry(bus_name.to_string()).or_default();
    bus_frames.insert(frame_key_from_frame(frame), frame.clone());
}

pub(super) fn parse_data_hex(raw: &str) -> Result<Vec<u8>, String> {
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

pub(super) fn validate_can_frame(bus: &SimCanBusDesc, frame: &SimCanFrame) -> Result<(), String> {
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

    let fd_requested = (frame.flags & CAN_FLAG_FD) != 0
        || frame.len > 8
        || (frame.flags & (CAN_FLAG_BRS | CAN_FLAG_ESI)) != 0;
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

pub(super) fn ensure_absolute_path(path: &str, context: &str) -> Result<(), String> {
    if Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(format!("{context} path must be absolute: '{path}'"))
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_absolute_path;

    #[test]
    fn ensure_absolute_path_accepts_absolute_paths() {
        let absolute = std::env::temp_dir()
            .join("agent-sim")
            .join("file.dbc")
            .to_string_lossy()
            .into_owned();
        assert!(ensure_absolute_path(&absolute, "DBC").is_ok());
    }

    #[test]
    fn ensure_absolute_path_rejects_relative_paths() {
        let err =
            ensure_absolute_path("dbc/internal.dbc", "DBC").expect_err("relative path must fail");
        assert!(
            err.contains("path must be absolute"),
            "unexpected error: {err}"
        );
    }
}
