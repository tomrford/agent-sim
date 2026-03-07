use super::DaemonState;
use crate::can::dbc::frame_key_from_frame;
use crate::sim::types::{SimCanBusDesc, SimCanFrame};

pub(super) fn process_can_rx(state: &mut DaemonState) -> Result<(), String> {
    let mut frame_updates = Vec::new();
    for (bus_name, attachment) in &mut state.can_attached {
        let frames = attachment.socket.recv_all()?;
        if frames.is_empty() {
            continue;
        }
        for frame in &frames {
            crate::can::validate_frame(&attachment.meta.name, attachment.meta.fd_capable, frame)?;
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
            crate::can::validate_frame(&attachment.meta.name, attachment.meta.fd_capable, &frame)?;
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
