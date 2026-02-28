use serde::{Deserialize, Serialize};
use std::ffi::c_char;

#[repr(C)]
pub struct SimCtx {
    _private: [u8; 0],
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimStatusRaw {
    Ok = 0,
    InvalidCtx = 1,
    InvalidArg = 2,
    InvalidSignal = 3,
    TypeMismatch = 4,
    BufferTooSmall = 5,
    Internal = 255,
}

impl TryFrom<u32> for SimStatusRaw {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Ok),
            1 => Ok(Self::InvalidCtx),
            2 => Ok(Self::InvalidArg),
            3 => Ok(Self::InvalidSignal),
            4 => Ok(Self::TypeMismatch),
            5 => Ok(Self::BufferTooSmall),
            255 => Ok(Self::Internal),
            _ => Err(()),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimTypeRaw {
    Bool = 0,
    U32 = 1,
    I32 = 2,
    F32 = 3,
    F64 = 4,
}

impl TryFrom<u32> for SimTypeRaw {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Bool),
            1 => Ok(Self::U32),
            2 => Ok(Self::I32),
            3 => Ok(Self::F32),
            4 => Ok(Self::F64),
            _ => Err(()),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union SimValueDataRaw {
    pub b: bool,
    pub u32: u32,
    pub i32: i32,
    pub f32: f32,
    pub f64: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SimValueRaw {
    pub signal_type: u32,
    pub data: SimValueDataRaw,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SimSignalDescRaw {
    pub id: u32,
    pub name: *const c_char,
    pub signal_type: u32,
    pub units: *const c_char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Bool,
    U32,
    I32,
    F32,
    F64,
}

impl SignalType {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bool" => Some(Self::Bool),
            "u32" => Some(Self::U32),
            "i32" => Some(Self::I32),
            "f32" => Some(Self::F32),
            "f64" => Some(Self::F64),
            _ => None,
        }
    }
}

impl std::fmt::Display for SignalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Bool => "bool",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::F32 => "f32",
            Self::F64 => "f64",
        };
        write!(f, "{value}")
    }
}

impl TryFrom<u32> for SignalType {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match SimTypeRaw::try_from(value)? {
            SimTypeRaw::Bool => SignalType::Bool,
            SimTypeRaw::U32 => SignalType::U32,
            SimTypeRaw::I32 => SignalType::I32,
            SimTypeRaw::F32 => SignalType::F32,
            SimTypeRaw::F64 => SignalType::F64,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SignalValue {
    Bool(bool),
    U32(u32),
    I32(i32),
    F32(f32),
    F64(f64),
}

impl SignalValue {
    pub fn signal_type(&self) -> SignalType {
        match self {
            Self::Bool(_) => SignalType::Bool,
            Self::U32(_) => SignalType::U32,
            Self::I32(_) => SignalType::I32,
            Self::F32(_) => SignalType::F32,
            Self::F64(_) => SignalType::F64,
        }
    }

    pub fn to_raw(&self) -> SimValueRaw {
        match self {
            Self::Bool(v) => SimValueRaw {
                signal_type: SimTypeRaw::Bool as u32,
                data: SimValueDataRaw { b: *v },
            },
            Self::U32(v) => SimValueRaw {
                signal_type: SimTypeRaw::U32 as u32,
                data: SimValueDataRaw { u32: *v },
            },
            Self::I32(v) => SimValueRaw {
                signal_type: SimTypeRaw::I32 as u32,
                data: SimValueDataRaw { i32: *v },
            },
            Self::F32(v) => SimValueRaw {
                signal_type: SimTypeRaw::F32 as u32,
                data: SimValueDataRaw { f32: *v },
            },
            Self::F64(v) => SimValueRaw {
                signal_type: SimTypeRaw::F64 as u32,
                data: SimValueDataRaw { f64: *v },
            },
        }
    }

    pub unsafe fn from_raw(raw: SimValueRaw) -> Option<Self> {
        match SimTypeRaw::try_from(raw.signal_type).ok()? {
            SimTypeRaw::Bool => Some(Self::Bool(unsafe { raw.data.b })),
            SimTypeRaw::U32 => Some(Self::U32(unsafe { raw.data.u32 })),
            SimTypeRaw::I32 => Some(Self::I32(unsafe { raw.data.i32 })),
            SimTypeRaw::F32 => Some(Self::F32(unsafe { raw.data.f32 })),
            SimTypeRaw::F64 => Some(Self::F64(unsafe { raw.data.f64 })),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignalMeta {
    pub id: u32,
    pub name: String,
    pub signal_type: SignalType,
    pub units: Option<String>,
}
