use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonError {
    #[error("daemon startup timeout")]
    StartupTimeout,
    #[error("daemon startup failed: {0}")]
    StartupFailed(String),
    #[error("session '{0}' already has a running daemon")]
    AlreadyRunning(String),
    #[error("session '{0}' has no running daemon; run `agent-sim load <libpath>` first")]
    NotRunning(String),
    #[error("daemon request failed: {0}")]
    Request(String),
    #[error("daemon io error: {0}")]
    Io(#[from] std::io::Error),
}
