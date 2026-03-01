use crate::sim::types::{SignalType, SimStatusRaw};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("project already loaded")]
    AlreadyLoaded,
    #[error("project not loaded")]
    NotLoaded,
    #[error("library load failed: {0}")]
    LibraryLoad(String),
    #[error("missing symbol: {0}")]
    MissingSymbol(&'static str),
    #[error("invalid signal metadata from project")]
    InvalidSignalMetadata,
}

#[derive(Debug, Error)]
pub enum SimError {
    #[error("invalid instance context (freed or corrupted)")]
    InvalidCtx,
    #[error("invalid argument: {0}")]
    InvalidArg(String),
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
pub enum InstanceError {
    #[error("no active instance")]
    NoActiveInstance,
    #[error("instance index out of range: {0}")]
    IndexOutOfRange(u32),
}

#[derive(Debug, Error)]
pub enum TimeError {
    #[error("time engine requires loaded project")]
    ProjectNotLoaded,
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
            SimStatusRaw::InvalidCtx => SimError::InvalidCtx,
            SimStatusRaw::InvalidArg => SimError::InvalidArg("invalid ffi argument".to_string()),
            SimStatusRaw::InvalidSignal => SimError::InvalidSignal("<unknown>".to_string()),
            SimStatusRaw::TypeMismatch => SimError::Internal,
            SimStatusRaw::BufferTooSmall => SimError::BufferTooSmall,
            SimStatusRaw::Internal => SimError::Internal,
        })
    }
}
