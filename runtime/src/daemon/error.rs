use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("daemon startup timeout")]
    StartupTimeout,
    #[error("daemon request failed: {0}")]
    Request(String),
    #[error("daemon io error: {0}")]
    Io(#[from] std::io::Error),
}
