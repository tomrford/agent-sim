use crate::load::ResolvedFlashRegion;
use crate::sim::error::{ProjectError, SimError};
use crate::sim::project_loader::{decode_owned_cstr, next_capacity, validate_written};
use crate::sim::types::{
    SignalMeta, SignalType, SignalValue, SimCanBusDesc, SimCanBusDescRaw, SimCanFrame,
    SimCanFrameRaw, SimSharedDesc, SimSharedDescRaw, SimSharedSlot, SimSharedSlotRaw,
    SimSignalDescRaw, SimValueRaw,
};
use crate::sim::validation::{
    validate_can_metadata, validate_shared_metadata, validate_signal_metadata,
};
use libloading::Library;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

type SimInitFn = unsafe extern "C" fn() -> u32;
type SimResetFn = unsafe extern "C" fn() -> u32;
type SimTickFn = unsafe extern "C" fn() -> u32;
type SimReadValFn = unsafe extern "C" fn(u32, *mut SimValueRaw) -> u32;
type SimWriteValFn = unsafe extern "C" fn(u32, *const SimValueRaw) -> u32;
type SimGetSignalCountFn = unsafe extern "C" fn(*mut u32) -> u32;
type SimGetSignalsFn = unsafe extern "C" fn(*mut SimSignalDescRaw, u32, *mut u32) -> u32;
type SimGetApiVersionFn = unsafe extern "C" fn(*mut u32, *mut u32) -> u32;
type SimGetTickDurationUsFn = unsafe extern "C" fn(*mut u32) -> u32;
type SimFlashWriteFn = unsafe extern "C" fn(u32, *const u8, u32) -> u32;
type SimCanGetBusesFn = unsafe extern "C" fn(*mut SimCanBusDescRaw, u32, *mut u32) -> u32;
type SimCanRxFn = unsafe extern "C" fn(u32, *const SimCanFrameRaw, u32) -> u32;
type SimCanTxFn = unsafe extern "C" fn(u32, *mut SimCanFrameRaw, u32, *mut u32) -> u32;
type SimSharedGetChannelsFn = unsafe extern "C" fn(*mut SimSharedDescRaw, u32, *mut u32) -> u32;
type SimSharedReadFn =
    unsafe extern "C" fn(u32, *const crate::sim::types::SimSharedSlotRaw, u32) -> u32;
type SimSharedWriteFn =
    unsafe extern "C" fn(u32, *mut crate::sim::types::SimSharedSlotRaw, u32, *mut u32) -> u32;

const STATUS_OK: u32 = 0;
const STATUS_NOT_INITIALIZED: u32 = 1;
const STATUS_INVALID_SIGNAL: u32 = 3;
const STATUS_TYPE_MISMATCH: u32 = 4;
const STATUS_BUFFER_TOO_SMALL: u32 = 5;

fn validate_tick_duration_us(
    tick_duration_us: u32,
    source: &str,
) -> Result<u32, crate::sim::error::ProjectError> {
    if tick_duration_us == 0 {
        return Err(crate::sim::error::ProjectError::LibraryLoad(format!(
            "{source} returned invalid zero tick duration"
        )));
    }
    Ok(tick_duration_us)
}

const SUPPORTED_API_VERSION_MAJOR: u32 = 2;
const SUPPORTED_API_VERSION_MINOR: u32 = 0;

struct ProjectCanApi {
    sim_can_rx: SimCanRxFn,
    sim_can_tx: SimCanTxFn,
}

struct ProjectSharedApi {
    sim_shared_read: SimSharedReadFn,
    sim_shared_write: SimSharedWriteFn,
}

pub struct Project {
    pub libpath: PathBuf,
    tick_duration_us: u32,
    signals: Vec<SignalMeta>,
    can_buses: Vec<SimCanBusDesc>,
    shared_channels: Vec<SimSharedDesc>,
    signal_name_to_id: HashMap<String, u32>,
    signal_id_to_index: HashMap<u32, usize>,
    sim_reset: SimResetFn,
    sim_tick: SimTickFn,
    sim_read_val: SimReadValFn,
    sim_write_val: SimWriteValFn,
    _sim_get_signal_count: SimGetSignalCountFn,
    _sim_get_signals: SimGetSignalsFn,
    _sim_get_tick_duration_us: SimGetTickDurationUsFn,
    can_api: Option<ProjectCanApi>,
    shared_api: Option<ProjectSharedApi>,
    _library: Library,
}

impl Project {
    pub fn load(
        libpath: impl AsRef<Path>,
        flash_regions: &[ResolvedFlashRegion],
    ) -> Result<Self, ProjectError> {
        let path = libpath.as_ref().to_path_buf();
        let library =
            unsafe { Library::new(&path) }.map_err(|e| ProjectError::LibraryLoad(e.to_string()))?;

        let sim_init: SimInitFn = *unsafe { library.get::<SimInitFn>(b"sim_init\0") }
            .map_err(|_| ProjectError::MissingSymbol("sim_init"))?;
        let sim_reset: SimResetFn = *unsafe { library.get::<SimResetFn>(b"sim_reset\0") }
            .map_err(|_| ProjectError::MissingSymbol("sim_reset"))?;
        let sim_tick: SimTickFn = *unsafe { library.get::<SimTickFn>(b"sim_tick\0") }
            .map_err(|_| ProjectError::MissingSymbol("sim_tick"))?;
        let sim_read_val: SimReadValFn = *unsafe { library.get::<SimReadValFn>(b"sim_read_val\0") }
            .map_err(|_| ProjectError::MissingSymbol("sim_read_val"))?;
        let sim_write_val: SimWriteValFn =
            *unsafe { library.get::<SimWriteValFn>(b"sim_write_val\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_write_val"))?;
        let sim_get_signal_count: SimGetSignalCountFn =
            *unsafe { library.get::<SimGetSignalCountFn>(b"sim_get_signal_count\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_get_signal_count"))?;
        let sim_get_signals: SimGetSignalsFn =
            *unsafe { library.get::<SimGetSignalsFn>(b"sim_get_signals\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_get_signals"))?;
        let sim_get_api_version: SimGetApiVersionFn =
            *unsafe { library.get::<SimGetApiVersionFn>(b"sim_get_api_version\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_get_api_version"))?;
        let sim_get_tick_duration_us: SimGetTickDurationUsFn =
            *unsafe { library.get::<SimGetTickDurationUsFn>(b"sim_get_tick_duration_us\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_get_tick_duration_us"))?;
        let sim_flash_write = unsafe { library.get::<SimFlashWriteFn>(b"sim_flash_write\0") }
            .ok()
            .map(|symbol| *symbol);
        let sim_can_get_buses = unsafe { library.get::<SimCanGetBusesFn>(b"sim_can_get_buses\0") }
            .ok()
            .map(|symbol| *symbol);
        let sim_can_rx = unsafe { library.get::<SimCanRxFn>(b"sim_can_rx\0") }
            .ok()
            .map(|symbol| *symbol);
        let sim_can_tx = unsafe { library.get::<SimCanTxFn>(b"sim_can_tx\0") }
            .ok()
            .map(|symbol| *symbol);
        let sim_shared_get_channels =
            unsafe { library.get::<SimSharedGetChannelsFn>(b"sim_shared_get_channels\0") }
                .ok()
                .map(|symbol| *symbol);
        let sim_shared_read = unsafe { library.get::<SimSharedReadFn>(b"sim_shared_read\0") }
            .ok()
            .map(|symbol| *symbol);
        let sim_shared_write = unsafe { library.get::<SimSharedWriteFn>(b"sim_shared_write\0") }
            .ok()
            .map(|symbol| *symbol);

        {
            let mut major = 0_u32;
            let mut minor = 0_u32;
            let status =
                unsafe { sim_get_api_version(&mut major as *mut u32, &mut minor as *mut u32) };
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_get_api_version failed with status {status}"
                )));
            }
            if major != SUPPORTED_API_VERSION_MAJOR || minor != SUPPORTED_API_VERSION_MINOR {
                return Err(ProjectError::LibraryLoad(format!(
                    "ABI version mismatch: project reports {major}.{minor}, runtime requires {}.{}",
                    SUPPORTED_API_VERSION_MAJOR, SUPPORTED_API_VERSION_MINOR
                )));
            }
        }

        let tick_duration_us = {
            let mut value = 0_u32;
            let status = unsafe { sim_get_tick_duration_us(&mut value as *mut u32) };
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_get_tick_duration_us failed with status {status}"
                )));
            }
            validate_tick_duration_us(value, "sim_get_tick_duration_us")?
        };

        let signals = {
            let mut count = 0_u32;
            let status = unsafe { sim_get_signal_count(&mut count as *mut u32) };
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_get_signal_count failed with status {status}"
                )));
            }

            let mut capacity = count.max(1);
            loop {
                let mut raw = vec![
                    SimSignalDescRaw {
                        id: 0,
                        name: std::ptr::null(),
                        signal_type: 0,
                        units: std::ptr::null(),
                    };
                    capacity as usize
                ];
                let mut written = 0_u32;
                let status = unsafe {
                    sim_get_signals(raw.as_mut_ptr(), capacity, &mut written as *mut u32)
                };
                if status == STATUS_BUFFER_TOO_SMALL {
                    capacity = next_capacity(capacity, "sim_get_signals")
                        .map_err(ProjectError::FfiContract)?;
                    continue;
                }
                if status != STATUS_OK {
                    return Err(ProjectError::LibraryLoad(format!(
                        "sim_get_signals failed with status {status}"
                    )));
                }
                raw.truncate(
                    validate_written(written, capacity, "sim_get_signals")
                        .map_err(ProjectError::FfiContract)?,
                );
                break raw
                    .into_iter()
                    .map(|entry| {
                        let name = decode_owned_cstr(entry.name, "signal name")
                            .map_err(ProjectError::InvalidSignalMetadata)?;
                        let units = if entry.units.is_null() {
                            None
                        } else {
                            Some(
                                decode_owned_cstr(entry.units, "signal units")
                                    .map_err(ProjectError::InvalidSignalMetadata)?,
                            )
                        };
                        let signal_type =
                            SignalType::try_from(entry.signal_type).map_err(|_| {
                                ProjectError::InvalidSignalMetadata(format!(
                                    "signal '{}' uses invalid type tag {}",
                                    name, entry.signal_type
                                ))
                            })?;
                        Ok(SignalMeta {
                            id: entry.id,
                            name,
                            signal_type,
                            units,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
            }
        };

        if !flash_regions.is_empty() {
            let flash_write = sim_flash_write.ok_or_else(|| {
                ProjectError::Flash(
                    "flash regions were configured, but the project does not export sim_flash_write"
                        .to_string(),
                )
            })?;
            for region in flash_regions {
                let len = u32::try_from(region.data.len()).map_err(|_| {
                    ProjectError::Flash(format!(
                        "flash region at 0x{:08X} is too large ({} bytes)",
                        region.base_addr,
                        region.data.len()
                    ))
                })?;
                let status = unsafe { flash_write(region.base_addr, region.data.as_ptr(), len) };
                if status != STATUS_OK {
                    return Err(ProjectError::Flash(format!(
                        "sim_flash_write failed for region 0x{:08X} ({} bytes) with status {}",
                        region.base_addr,
                        region.data.len(),
                        status
                    )));
                }
            }
        }

        let init_status = unsafe { sim_init() };
        if init_status != STATUS_OK {
            return Err(ProjectError::LibraryLoad(format!(
                "sim_init failed with status {init_status}"
            )));
        }

        let (can_api, can_buses) = match (sim_can_get_buses, sim_can_rx, sim_can_tx) {
            (None, None, None) => (None, Vec::new()),
            (Some(get_buses), Some(can_rx), Some(can_tx)) => (
                Some(ProjectCanApi {
                    sim_can_rx: can_rx,
                    sim_can_tx: can_tx,
                }),
                Self::load_can_buses(get_buses)?,
            ),
            _ => {
                return Err(ProjectError::InvalidCanExports(
                    "if any CAN symbol is exported, sim_can_get_buses/sim_can_rx/sim_can_tx must all be exported"
                        .to_string(),
                ));
            }
        };

        let (shared_api, shared_channels) = match (
            sim_shared_get_channels,
            sim_shared_read,
            sim_shared_write,
        ) {
            (None, None, None) => (None, Vec::new()),
            (Some(get_channels), Some(shared_read), Some(shared_write)) => (
                Some(ProjectSharedApi {
                    sim_shared_read: shared_read,
                    sim_shared_write: shared_write,
                }),
                Self::load_shared_channels(get_channels)?,
            ),
            _ => {
                return Err(ProjectError::InvalidSharedExports(
                            "if any shared-state symbol is exported, sim_shared_get_channels/sim_shared_read/sim_shared_write must all be exported"
                                .to_string(),
                        ));
            }
        };
        validate_signal_metadata(&signals)?;
        validate_can_metadata(&can_buses)?;
        validate_shared_metadata(&shared_channels)?;

        let signal_name_to_id = signals
            .iter()
            .map(|s| (s.name.clone(), s.id))
            .collect::<HashMap<_, _>>();
        let signal_id_to_index = signals
            .iter()
            .enumerate()
            .map(|(idx, s)| (s.id, idx))
            .collect::<HashMap<_, _>>();

        Ok(Self {
            libpath: path,
            tick_duration_us,
            signals,
            can_buses,
            shared_channels,
            signal_name_to_id,
            signal_id_to_index,
            sim_reset,
            sim_tick,
            sim_read_val,
            sim_write_val,
            _sim_get_signal_count: sim_get_signal_count,
            _sim_get_signals: sim_get_signals,
            _sim_get_tick_duration_us: sim_get_tick_duration_us,
            can_api,
            shared_api,
            _library: library,
        })
    }

    pub fn tick_duration_us(&self) -> u32 {
        self.tick_duration_us
    }

    pub fn signals(&self) -> &[SignalMeta] {
        &self.signals
    }

    pub fn can_buses(&self) -> &[SimCanBusDesc] {
        &self.can_buses
    }

    pub fn shared_channels(&self) -> &[SimSharedDesc] {
        &self.shared_channels
    }

    pub fn signal_by_id(&self, id: u32) -> Option<&SignalMeta> {
        self.signal_id_to_index
            .get(&id)
            .and_then(|idx| self.signals.get(*idx))
    }

    pub fn signal_id_by_name(&self, name: &str) -> Option<u32> {
        self.signal_name_to_id.get(name).copied()
    }

    fn shared_channel_by_id(&self, channel_id: u32) -> Option<&SimSharedDesc> {
        self.shared_channels
            .iter()
            .find(|channel| channel.id == channel_id)
    }

    pub(crate) fn reset(&self) -> Result<(), SimError> {
        self.map_status(unsafe { (self.sim_reset)() }, None, None)
    }

    pub(crate) fn tick(&self) -> Result<(), SimError> {
        self.map_status(unsafe { (self.sim_tick)() }, None, None)
    }

    pub(crate) fn can_rx(&self, bus_id: u32, frames: &[SimCanFrame]) -> Result<(), SimError> {
        let Some(can_api) = &self.can_api else {
            return Ok(());
        };
        if frames.is_empty() {
            return Ok(());
        }
        let raw_frames = frames.iter().map(SimCanFrame::to_raw).collect::<Vec<_>>();
        let status =
            unsafe { (can_api.sim_can_rx)(bus_id, raw_frames.as_ptr(), raw_frames.len() as u32) };
        self.map_status(status, None, None)
    }

    pub(crate) fn can_tx(&self, bus_id: u32) -> Result<Vec<SimCanFrame>, SimError> {
        let Some(can_api) = &self.can_api else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut capacity = 32_u32;
        loop {
            let mut raw_frames = vec![
                SimCanFrameRaw {
                    arb_id: 0,
                    len: 0,
                    flags: 0,
                    _pad: [0, 0],
                    data: [0; 64],
                };
                capacity as usize
            ];
            let mut written = 0_u32;
            let status = unsafe {
                (can_api.sim_can_tx)(
                    bus_id,
                    raw_frames.as_mut_ptr(),
                    capacity,
                    &mut written as *mut u32,
                )
            };
            if status != STATUS_OK && status != STATUS_BUFFER_TOO_SMALL {
                self.map_status(status, None, None)?;
                break;
            }
            raw_frames.truncate(
                validate_written(written, capacity, "sim_can_tx").map_err(SimError::FfiContract)?,
            );
            out.extend(raw_frames.into_iter().map(SimCanFrame::from_raw));
            if status == STATUS_BUFFER_TOO_SMALL {
                capacity = next_capacity(capacity, "sim_can_tx").map_err(SimError::FfiContract)?;
                continue;
            }
            break;
        }
        Ok(out)
    }

    pub(crate) fn shared_read(
        &self,
        channel_id: u32,
        slots: &[SimSharedSlot],
    ) -> Result<(), SimError> {
        let Some(shared_api) = &self.shared_api else {
            return Ok(());
        };
        let expected_slot_count = self
            .shared_channel_by_id(channel_id)
            .ok_or_else(|| {
                SimError::FfiContract(format!("unknown shared channel id {channel_id}"))
            })?
            .slot_count as usize;
        validate_dense_shared_snapshot(slots, expected_slot_count, "sim_shared_read")?;
        let raw_slots = slots.iter().map(SimSharedSlot::to_raw).collect::<Vec<_>>();
        let status = unsafe {
            (shared_api.sim_shared_read)(channel_id, raw_slots.as_ptr(), raw_slots.len() as u32)
        };
        self.map_status(status, None, None)
    }

    pub(crate) fn shared_write(&self, channel_id: u32) -> Result<Vec<SimSharedSlot>, SimError> {
        let Some(shared_api) = &self.shared_api else {
            return Ok(Vec::new());
        };
        let expected_slot_count = self
            .shared_channel_by_id(channel_id)
            .ok_or_else(|| {
                SimError::FfiContract(format!("unknown shared channel id {channel_id}"))
            })?
            .slot_count;
        let capacity = expected_slot_count.max(1);
        let mut raw_slots = vec![SimSharedSlotRaw::default(); capacity as usize];
        let mut written = 0_u32;
        let status = unsafe {
            (shared_api.sim_shared_write)(
                channel_id,
                raw_slots.as_mut_ptr(),
                capacity,
                &mut written as *mut u32,
            )
        };
        if status == STATUS_BUFFER_TOO_SMALL {
            return Err(SimError::FfiContract(format!(
                "sim_shared_write reported BUFFER_TOO_SMALL for channel {channel_id} with declared slot_count {expected_slot_count}"
            )));
        }
        if status != STATUS_OK {
            self.map_status(status, None, None)?;
        }

        raw_slots.truncate(
            validate_written(written, capacity, "sim_shared_write")
                .map_err(SimError::FfiContract)?,
        );
        let slots = raw_slots
            .into_iter()
            .map(|slot| {
                SimSharedSlot::try_from_raw(slot)
                    .map_err(|err| SimError::FfiContract(format!("sim_shared_write: {err}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_dense_shared_snapshot(&slots, expected_slot_count as usize, "sim_shared_write")?;
        Ok(slots)
    }

    pub(crate) fn read(&self, signal: &SignalMeta) -> Result<SignalValue, SimError> {
        let mut raw = SimValueRaw {
            signal_type: 0,
            data: crate::sim::types::SimValueDataRaw { u32: 0 },
        };
        let status = unsafe { (self.sim_read_val)(signal.id, &mut raw as *mut SimValueRaw) };
        self.map_status(status, Some(signal), None)?;
        let value = unsafe { SignalValue::from_raw(raw) }
            .ok_or_else(|| SimError::InvalidArg("bad read value".to_string()))?;
        Ok(value)
    }

    pub(crate) fn write(&self, signal: &SignalMeta, value: &SignalValue) -> Result<(), SimError> {
        let raw = value.to_raw();
        let status = unsafe { (self.sim_write_val)(signal.id, &raw as *const SimValueRaw) };
        self.map_status(status, Some(signal), Some(value.signal_type()))
    }

    fn map_status(
        &self,
        status: u32,
        signal: Option<&SignalMeta>,
        actual_type: Option<SignalType>,
    ) -> Result<(), SimError> {
        match status {
            STATUS_OK => Ok(()),
            STATUS_NOT_INITIALIZED => Err(SimError::NotInitialized),
            2 => Err(SimError::InvalidArg("invalid ffi argument".to_string())),
            STATUS_INVALID_SIGNAL => Err(SimError::InvalidSignal(
                signal
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "<unknown>".to_string()),
            )),
            STATUS_TYPE_MISMATCH => Err(SimError::TypeMismatch {
                name: signal
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                expected: signal.map(|s| s.signal_type).unwrap_or(SignalType::F64),
                actual: actual_type.unwrap_or(SignalType::F64),
            }),
            STATUS_BUFFER_TOO_SMALL => Err(SimError::BufferTooSmall),
            255 => Err(SimError::Internal),
            _ => Err(SimError::UnknownStatus(status)),
        }
    }

    fn load_can_buses(
        sim_can_get_buses: SimCanGetBusesFn,
    ) -> Result<Vec<SimCanBusDesc>, ProjectError> {
        let mut capacity = 4_u32;
        loop {
            let mut raw = vec![
                SimCanBusDescRaw {
                    id: 0,
                    name: std::ptr::null(),
                    bitrate: 0,
                    bitrate_data: 0,
                    flags: 0,
                    _pad: [0, 0, 0],
                };
                capacity as usize
            ];
            let mut written = 0_u32;
            let status =
                unsafe { sim_can_get_buses(raw.as_mut_ptr(), capacity, &mut written as *mut u32) };
            if status == STATUS_BUFFER_TOO_SMALL {
                capacity = next_capacity(capacity, "sim_can_get_buses")
                    .map_err(ProjectError::FfiContract)?;
                continue;
            }
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_can_get_buses failed with status {status}"
                )));
            }
            raw.truncate(
                validate_written(written, capacity, "sim_can_get_buses")
                    .map_err(ProjectError::FfiContract)?,
            );
            return raw
                .into_iter()
                .map(|entry| {
                    let name = decode_owned_cstr(entry.name, "CAN bus name")
                        .map_err(ProjectError::InvalidCanMetadata)?;
                    Ok(SimCanBusDesc {
                        id: entry.id,
                        name,
                        bitrate: entry.bitrate,
                        bitrate_data: entry.bitrate_data,
                        fd_capable: (entry.flags & 0x01) != 0,
                    })
                })
                .collect();
        }
    }

    fn load_shared_channels(
        sim_shared_get_channels: SimSharedGetChannelsFn,
    ) -> Result<Vec<SimSharedDesc>, ProjectError> {
        let mut capacity = 4_u32;
        loop {
            let mut raw = vec![
                SimSharedDescRaw {
                    id: 0,
                    name: std::ptr::null(),
                    slot_count: 0,
                };
                capacity as usize
            ];
            let mut written = 0_u32;
            let status = unsafe {
                sim_shared_get_channels(raw.as_mut_ptr(), capacity, &mut written as *mut u32)
            };
            if status == STATUS_BUFFER_TOO_SMALL {
                capacity = next_capacity(capacity, "sim_shared_get_channels")
                    .map_err(ProjectError::FfiContract)?;
                continue;
            }
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_shared_get_channels failed with status {status}"
                )));
            }
            raw.truncate(
                validate_written(written, capacity, "sim_shared_get_channels")
                    .map_err(ProjectError::FfiContract)?,
            );
            return raw
                .into_iter()
                .map(|entry| {
                    let name = decode_owned_cstr(entry.name, "shared channel name")
                        .map_err(ProjectError::InvalidSharedMetadata)?;
                    Ok(SimSharedDesc {
                        id: entry.id,
                        name,
                        slot_count: entry.slot_count,
                    })
                })
                .collect();
        }
    }
}

fn validate_dense_shared_snapshot(
    slots: &[SimSharedSlot],
    expected_slot_count: usize,
    context: &str,
) -> Result<(), SimError> {
    if slots.len() != expected_slot_count {
        return Err(SimError::FfiContract(format!(
            "{context} returned {} slots, expected {expected_slot_count}",
            slots.len()
        )));
    }

    for (expected_slot_id, slot) in slots.iter().enumerate() {
        if slot.slot_id as usize != expected_slot_id {
            return Err(SimError::FfiContract(format!(
                "{context} returned slot id {} at dense index {}; expected slot id {}",
                slot.slot_id, expected_slot_id, expected_slot_id
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_tick_duration_us;
    use crate::sim::error::ProjectError;

    #[test]
    fn validate_tick_duration_rejects_zero() {
        let err = validate_tick_duration_us(0, "sim_get_tick_duration_us")
            .expect_err("zero tick duration must fail");
        assert!(
            matches!(err, ProjectError::LibraryLoad(message) if message.contains("invalid zero tick duration"))
        );
    }

    #[test]
    fn validate_tick_duration_accepts_positive_value() {
        assert_eq!(
            validate_tick_duration_us(20, "sim_get_tick_duration_us")
                .expect("positive tick duration must pass"),
            20
        );
    }
}
