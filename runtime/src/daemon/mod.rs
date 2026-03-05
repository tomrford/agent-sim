pub mod error;
pub mod lifecycle;
pub mod server;

use crate::daemon::lifecycle::socket_path;
use crate::error::AgentSimError;
use crate::sim::project::Project;

pub async fn run(
    session: &str,
    libpath: &str,
    env_tag: Option<String>,
) -> Result<(), AgentSimError> {
    let socket = socket_path(session);
    let project = Project::load(libpath)?;
    server::run_listener(session.to_string(), socket, project, env_tag)
        .await
        .map_err(AgentSimError::from)
}
