use crate::cli::error::CliError;
use crate::config::error::ConfigError;
use crate::connection::ConnectionError;
use crate::daemon::error::DaemonError;
use crate::protocol::ProtocolError;
use crate::sim::error::{ProjectError, SimError, TimeError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentSimError {
    #[error(transparent)]
    Cli(#[from] CliError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Connection(#[from] ConnectionError),
    #[error(transparent)]
    Daemon(#[from] DaemonError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Sim(#[from] SimError),
    #[error(transparent)]
    Time(#[from] TimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
