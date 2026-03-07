use crate::sim::error::ProjectError;
use crate::sim::types::{SignalMeta, SimCanBusDesc, SimSharedDesc};
use std::collections::HashSet;

pub fn validate_signal_metadata(signals: &[SignalMeta]) -> Result<(), ProjectError> {
    let mut ids = HashSet::with_capacity(signals.len());
    let mut names = HashSet::with_capacity(signals.len());

    for signal in signals {
        if signal.name.trim().is_empty() {
            return Err(ProjectError::InvalidSignalMetadata(
                "signal names must be non-empty".to_string(),
            ));
        }
        if signal.name.starts_with("can.") {
            return Err(ProjectError::InvalidSignalMetadata(format!(
                "signal '{}' uses reserved namespace 'can.'",
                signal.name
            )));
        }
        if !ids.insert(signal.id) {
            return Err(ProjectError::InvalidSignalMetadata(format!(
                "duplicate signal id {}",
                signal.id
            )));
        }
        if !names.insert(signal.name.as_str()) {
            return Err(ProjectError::InvalidSignalMetadata(format!(
                "duplicate signal name '{}'",
                signal.name
            )));
        }
    }

    Ok(())
}

pub fn validate_can_metadata(buses: &[SimCanBusDesc]) -> Result<(), ProjectError> {
    let mut ids = HashSet::with_capacity(buses.len());
    let mut names = HashSet::with_capacity(buses.len());

    for bus in buses {
        if bus.name.trim().is_empty() {
            return Err(ProjectError::InvalidCanMetadata(
                "CAN bus names must be non-empty".to_string(),
            ));
        }
        if !ids.insert(bus.id) {
            return Err(ProjectError::InvalidCanMetadata(format!(
                "duplicate CAN bus id {}",
                bus.id
            )));
        }
        if !names.insert(bus.name.as_str()) {
            return Err(ProjectError::InvalidCanMetadata(format!(
                "duplicate CAN bus name '{}'",
                bus.name
            )));
        }
    }

    Ok(())
}

pub fn validate_shared_metadata(channels: &[SimSharedDesc]) -> Result<(), ProjectError> {
    let mut ids = HashSet::with_capacity(channels.len());
    let mut names = HashSet::with_capacity(channels.len());

    for channel in channels {
        if channel.name.trim().is_empty() {
            return Err(ProjectError::InvalidSharedMetadata(
                "shared channel names must be non-empty".to_string(),
            ));
        }
        if channel.slot_count == 0 {
            return Err(ProjectError::InvalidSharedMetadata(format!(
                "shared channel '{}' must declare at least one slot",
                channel.name
            )));
        }
        if !ids.insert(channel.id) {
            return Err(ProjectError::InvalidSharedMetadata(format!(
                "duplicate shared channel id {}",
                channel.id
            )));
        }
        if !names.insert(channel.name.as_str()) {
            return Err(ProjectError::InvalidSharedMetadata(format!(
                "duplicate shared channel name '{}'",
                channel.name
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_can_metadata, validate_shared_metadata, validate_signal_metadata};
    use crate::sim::error::ProjectError;
    use crate::sim::types::{SignalMeta, SignalType, SimCanBusDesc, SimSharedDesc};

    fn signal(id: u32, name: &str) -> SignalMeta {
        SignalMeta {
            id,
            name: name.to_string(),
            signal_type: SignalType::F32,
            units: None,
        }
    }

    fn bus(id: u32, name: &str) -> SimCanBusDesc {
        SimCanBusDesc {
            id,
            name: name.to_string(),
            bitrate: 500_000,
            bitrate_data: 0,
            fd_capable: false,
        }
    }

    fn channel(id: u32, name: &str, slot_count: u32) -> SimSharedDesc {
        SimSharedDesc {
            id,
            name: name.to_string(),
            slot_count,
        }
    }

    #[test]
    fn signal_validation_rejects_duplicate_id() {
        let err = validate_signal_metadata(&[signal(1, "a"), signal(1, "b")])
            .expect_err("duplicate ids must fail");
        assert!(
            matches!(err, ProjectError::InvalidSignalMetadata(message) if message.contains("duplicate signal id 1"))
        );
    }

    #[test]
    fn signal_validation_rejects_duplicate_name() {
        let err = validate_signal_metadata(&[signal(1, "a"), signal(2, "a")])
            .expect_err("duplicate names must fail");
        assert!(
            matches!(err, ProjectError::InvalidSignalMetadata(message) if message.contains("duplicate signal name 'a'"))
        );
    }

    #[test]
    fn signal_validation_rejects_reserved_namespace() {
        let err = validate_signal_metadata(&[signal(1, "can.speed")])
            .expect_err("reserved namespace must fail");
        assert!(
            matches!(err, ProjectError::InvalidSignalMetadata(message) if message.contains("reserved namespace"))
        );
    }

    #[test]
    fn can_validation_rejects_duplicate_name() {
        let err =
            validate_can_metadata(&[bus(1, "internal"), bus(2, "internal")]).expect_err("dup");
        assert!(
            matches!(err, ProjectError::InvalidCanMetadata(message) if message.contains("duplicate CAN bus name 'internal'"))
        );
    }

    #[test]
    fn can_validation_rejects_duplicate_id() {
        let err =
            validate_can_metadata(&[bus(7, "internal"), bus(7, "external")]).expect_err("dup");
        assert!(
            matches!(err, ProjectError::InvalidCanMetadata(message) if message.contains("duplicate CAN bus id 7"))
        );
    }

    #[test]
    fn shared_validation_rejects_zero_slots() {
        let err = validate_shared_metadata(&[channel(1, "sensor_feed", 0)])
            .expect_err("zero-slot shared channel must fail");
        assert!(
            matches!(err, ProjectError::InvalidSharedMetadata(message) if message.contains("at least one slot"))
        );
    }

    #[test]
    fn shared_validation_rejects_duplicate_id() {
        let err =
            validate_shared_metadata(&[channel(1, "sensor_feed", 2), channel(1, "other_feed", 2)])
                .expect_err("duplicate ids must fail");
        assert!(
            matches!(err, ProjectError::InvalidSharedMetadata(message) if message.contains("duplicate shared channel id 1"))
        );
    }

    #[test]
    fn shared_validation_rejects_duplicate_name() {
        let err =
            validate_shared_metadata(&[channel(1, "sensor_feed", 2), channel(2, "sensor_feed", 2)])
                .expect_err("duplicate names must fail");
        assert!(
            matches!(err, ProjectError::InvalidSharedMetadata(message) if message.contains("duplicate shared channel name 'sensor_feed'"))
        );
    }
}
