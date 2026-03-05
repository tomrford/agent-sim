pub mod error;
pub mod lifecycle;
pub mod server;
pub mod spec;

use crate::envd::lifecycle::socket_path;
use crate::envd::spec::EnvSpec;
use crate::error::AgentSimError;

pub async fn run(env_spec: EnvSpec) -> Result<(), AgentSimError> {
    let socket = socket_path(&env_spec.name);
    server::run_listener(socket, env_spec)
        .await
        .map_err(AgentSimError::from)
}
