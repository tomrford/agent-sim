use crate::sim::types::{SignalType, SimStatusRaw};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("library load failed: {0}")]
    LibraryLoad(String),
    #[error("FFI contract violation: {0}")]
    FfiContract(String),
    #[error("missing symbol: {0}")]
    MissingSymbol(&'static str),
    #[error("invalid signal metadata from project: {0}")]
    InvalidSignalMetadata(String),
    #[error("invalid CAN exports: {0}")]
    InvalidCanExports(String),
    #[error("invalid shared-state exports: {0}")]
    InvalidSharedExports(String),
    #[error("invalid CAN metadata from project: {0}")]
    InvalidCanMetadata(String),
    #[error("invalid shared-state metadata from project: {0}")]
    InvalidSharedMetadata(String),
    #[error("flash support error: {0}")]
    Flash(String),
}

#[derive(Debug, Error)]
pub enum SimError {
    #[error("simulation state not initialized")]
    NotInitialized,
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    #[error("FFI contract violation: {0}")]
    FfiContract(String),
    #[error("signal not found: '{0}'")]
    InvalidSignal(String),
    #[error("type mismatch: signal '{name}' expects {expected}, got {actual}")]
    TypeMismatch {
        name: String,
        expected: SignalType,
        actual: SignalType,
    },
    #[error("buffer too small")]
    BufferTooSmall,
    #[error("internal simulation error")]
    Internal,
    #[error("unknown simulation error status code: {0}")]
    UnknownStatus(u32),
}

#[derive(Debug, Error)]
pub enum TimeError {
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error("step while running is not allowed; pause first")]
    StepWhileRunning,
    #[error("time engine is already running")]
    AlreadyRunning,
    #[error("time engine is already paused")]
    AlreadyPaused,
    #[error("invalid speed multiplier: {0}")]
    InvalidSpeed(f64),
}

impl TryFrom<u32> for SimError {
    type Error = SimError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        let status = SimStatusRaw::try_from(value).map_err(|_| SimError::UnknownStatus(value))?;
        Ok(match status {
            SimStatusRaw::Ok => return Err(SimError::UnknownStatus(0)),
            SimStatusRaw::NotInitialized => SimError::NotInitialized,
            SimStatusRaw::InvalidArg => SimError::InvalidArg("invalid ffi argument".to_string()),
            SimStatusRaw::InvalidSignal => SimError::InvalidSignal("<unknown>".to_string()),
            SimStatusRaw::TypeMismatch => SimError::Internal,
            SimStatusRaw::BufferTooSmall => SimError::BufferTooSmall,
            SimStatusRaw::Internal => SimError::Internal,
        })
    }
}
