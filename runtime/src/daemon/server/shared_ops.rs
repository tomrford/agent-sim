use super::DaemonState;
use crate::sim::types::SimSharedDesc;

pub(super) fn process_shared_rx(state: &mut DaemonState) -> Result<(), String> {
    for (channel_name, attachment) in &state.shared_attached {
        let slots = attachment
            .region
            .read_snapshot()
            .map_err(|e| format!("failed reading shared channel '{channel_name}': {e}"))?;
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

pub(super) fn process_shared_tx(state: &mut DaemonState) -> Result<(), String> {
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

pub(super) fn find_shared_channel_meta(
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
