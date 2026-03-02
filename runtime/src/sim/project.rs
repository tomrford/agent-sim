use crate::sim::error::{ProjectError, SimError};
use crate::sim::types::{SignalMeta, SignalType, SignalValue, SimSignalDescRaw, SimValueRaw};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::CStr;
use std::path::{Path, PathBuf};

type SimInitFn = unsafe extern "C" fn() -> u32;
type SimResetFn = unsafe extern "C" fn() -> u32;
type SimTickFn = unsafe extern "C" fn() -> u32;
type SimReadValFn = unsafe extern "C" fn(u32, *mut SimValueRaw) -> u32;
type SimWriteValFn = unsafe extern "C" fn(u32, *const SimValueRaw) -> u32;
type SimGetSignalCountFn = unsafe extern "C" fn(*mut u32) -> u32;
type SimGetSignalsFn = unsafe extern "C" fn(*mut SimSignalDescRaw, u32, *mut u32) -> u32;
type SimGetTickDurationUsFn = unsafe extern "C" fn(*mut u32) -> u32;

const STATUS_OK: u32 = 0;
const STATUS_NOT_INITIALIZED: u32 = 1;
const STATUS_INVALID_SIGNAL: u32 = 3;
const STATUS_TYPE_MISMATCH: u32 = 4;
const STATUS_BUFFER_TOO_SMALL: u32 = 5;

pub struct Project {
    pub libpath: PathBuf,
    tick_duration_us: u32,
    signals: Vec<SignalMeta>,
    signal_name_to_id: HashMap<String, u32>,
    signal_id_to_index: HashMap<u32, usize>,
    sim_reset: SimResetFn,
    sim_tick: SimTickFn,
    sim_read_val: SimReadValFn,
    sim_write_val: SimWriteValFn,
    _sim_get_signal_count: SimGetSignalCountFn,
    _sim_get_signals: SimGetSignalsFn,
    _sim_get_tick_duration_us: SimGetTickDurationUsFn,
    _library: Library,
}

impl Project {
    pub fn load(libpath: impl AsRef<Path>) -> Result<Self, ProjectError> {
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
        let sim_get_tick_duration_us: SimGetTickDurationUsFn =
            *unsafe { library.get::<SimGetTickDurationUsFn>(b"sim_get_tick_duration_us\0") }
                .map_err(|_| ProjectError::MissingSymbol("sim_get_tick_duration_us"))?;

        let tick_duration_us = {
            let mut value = 0_u32;
            let status = unsafe { sim_get_tick_duration_us(&mut value as *mut u32) };
            if status != STATUS_OK {
                return Err(ProjectError::LibraryLoad(format!(
                    "sim_get_tick_duration_us failed with status {status}"
                )));
            }
            value
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
                    capacity = (capacity * 2).max(2);
                    continue;
                }
                if status != STATUS_OK {
                    return Err(ProjectError::LibraryLoad(format!(
                        "sim_get_signals failed with status {status}"
                    )));
                }
                raw.truncate(written as usize);
                break raw
                    .into_iter()
                    .map(|entry| {
                        let name = if entry.name.is_null() {
                            return Err(ProjectError::InvalidSignalMetadata);
                        } else {
                            unsafe { CStr::from_ptr(entry.name) }
                                .to_string_lossy()
                                .to_string()
                        };
                        let units = if entry.units.is_null() {
                            None
                        } else {
                            Some(
                                unsafe { CStr::from_ptr(entry.units) }
                                    .to_string_lossy()
                                    .to_string(),
                            )
                        };
                        let signal_type = SignalType::try_from(entry.signal_type)
                            .map_err(|_| ProjectError::InvalidSignalMetadata)?;
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

        let init_status = unsafe { sim_init() };
        if init_status != STATUS_OK {
            return Err(ProjectError::LibraryLoad(format!(
                "sim_init failed with status {init_status}"
            )));
        }

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
            signal_name_to_id,
            signal_id_to_index,
            sim_reset,
            sim_tick,
            sim_read_val,
            sim_write_val,
            _sim_get_signal_count: sim_get_signal_count,
            _sim_get_signals: sim_get_signals,
            _sim_get_tick_duration_us: sim_get_tick_duration_us,
            _library: library,
        })
    }

    pub fn tick_duration_us(&self) -> u32 {
        self.tick_duration_us
    }

    pub fn signals(&self) -> &[SignalMeta] {
        &self.signals
    }

    pub fn signal_by_id(&self, id: u32) -> Option<&SignalMeta> {
        self.signal_id_to_index
            .get(&id)
            .and_then(|idx| self.signals.get(*idx))
    }

    pub fn signal_id_by_name(&self, name: &str) -> Option<u32> {
        self.signal_name_to_id.get(name).copied()
    }

    pub(crate) fn reset(&self) -> Result<(), SimError> {
        self.map_status(unsafe { (self.sim_reset)() }, None, None)
    }

    pub(crate) fn tick(&self) -> Result<(), SimError> {
        self.map_status(unsafe { (self.sim_tick)() }, None, None)
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
}
