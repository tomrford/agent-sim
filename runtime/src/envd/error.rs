use thiserror::Error;

#[derive(Debug, Error)]
pub enum EnvDaemonError {
    #[error("env daemon startup timeout")]
    StartupTimeout,
    #[error("env daemon startup failed: {0}")]
    StartupFailed(String),
    #[error("env '{0}' already has a running daemon")]
    AlreadyRunning(String),
    #[error("env '{0}' has no running daemon; run `agent-sim env start <name>` first")]
    NotRunning(String),
    #[error("env daemon request failed: {0}")]
    Request(String),
    #[error("env daemon io error: {0}")]
    Io(#[from] std::io::Error),
}
