pub mod error;
pub mod lifecycle;
pub mod server;

use crate::daemon::lifecycle::socket_path;
use crate::error::AgentSimError;

pub async fn run(session: &str) -> Result<(), AgentSimError> {
    let socket = socket_path(session);
    server::run_listener(session.to_string(), socket)
        .await
        .map_err(AgentSimError::from)
}
