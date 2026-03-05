pub mod error;
pub mod lifecycle;
pub mod server;

use crate::daemon::lifecycle::socket_path;
use crate::error::AgentSimError;
use crate::load::LoadSpec;
use crate::sim::project::Project;

pub async fn run(session: &str, load_spec: LoadSpec) -> Result<(), AgentSimError> {
    let socket = socket_path(session);
    let env_tag = load_spec.env_tag.clone();
    let project = Project::load(&load_spec.libpath, &load_spec.flash)?;
    server::run_listener(session.to_string(), socket, project, env_tag)
        .await
        .map_err(AgentSimError::from)
}
