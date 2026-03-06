use super::DaemonState;
use crate::can::dbc::frame_key_from_frame;
use crate::sim::types::{
    CAN_FLAG_BRS, CAN_FLAG_ESI, CAN_FLAG_EXTENDED, CAN_FLAG_FD, CAN_FLAG_RESERVED_MASK,
    CAN_FLAG_RTR, SimCanBusDesc, SimCanFrame,
};

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

pub(super) fn record_frame(state: &mut DaemonState, bus_name: &str, frame: &SimCanFrame) {
    let bus_frames = state.frame_state.entry(bus_name.to_string()).or_default();
    bus_frames.insert(frame_key_from_frame(frame), frame.clone());
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

#[cfg(test)]
pub(super) fn ensure_absolute_path(path: &str, context: &str) -> Result<(), String> {
    if std::path::Path::new(path).is_absolute() {
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
