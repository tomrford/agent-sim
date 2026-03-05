use super::{response_error, send_action, send_action_success};
use crate::cli::args::{CliArgs, EnvArgs, EnvCommand};
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::{
    EnvCanBus, EnvDef, EnvSession, EnvSharedChannel, toml_value_to_signal_value,
};
use crate::daemon::lifecycle;
use crate::protocol::Action;
use crate::sim::init::InitEntry;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) async fn run_env_command(args: &CliArgs, env: &EnvArgs) -> Result<ExitCode, CliError> {
    match &env.command {
        EnvCommand::Start { name } => run_env_start(args, name).await,
    }
}

async fn run_env_start(args: &CliArgs, env_name: &str) -> Result<ExitCode, CliError> {
    let config = load_config(args.config.as_deref().map(Path::new))
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    let config_base_dir = config
        .source_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(Path::to_path_buf);
    let env_def = config
        .env(env_name)
        .map_err(|e| CliError::CommandFailed(e.to_string()))?
        .clone();
    validate_env_can_ifaces(&env_def)?;
    ensure_sessions_available(&env_def).await?;

    let mut started_sessions = Vec::new();
    let result = start_env_internal(
        env_name,
        &env_def,
        config_base_dir.as_deref(),
        &mut started_sessions,
    )
    .await;
    if let Err(err) = result {
        rollback_started_sessions(&started_sessions).await;
        return Err(err);
    }
    Ok(ExitCode::SUCCESS)
}

fn validate_env_can_ifaces(env_def: &EnvDef) -> Result<(), CliError> {
    #[cfg(target_os = "linux")]
    {
        for bus in env_def.can.values() {
            let iface_path = Path::new("/sys/class/net").join(&bus.vcan);
            if !iface_path.exists() {
                return Err(CliError::CommandFailed(format!(
                    "VCAN interface '{}' does not exist",
                    bus.vcan
                )));
            }
            let operstate_path = iface_path.join("operstate");
            let state = std::fs::read_to_string(&operstate_path)
                .unwrap_or_else(|_| "unknown".to_string())
                .trim()
                .to_string();
            if state != "up" && state != "unknown" {
                return Err(CliError::CommandFailed(format!(
                    "VCAN interface '{}' is not up (state: {state})",
                    bus.vcan
                )));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = env_def;
    }
    Ok(())
}

async fn ensure_sessions_available(env_def: &EnvDef) -> Result<(), CliError> {
    let sessions = lifecycle::list_sessions()
        .await
        .map_err(|e| CliError::CommandFailed(e.to_string()))?;
    for session in &env_def.sessions {
        if sessions
            .iter()
            .any(|(name, _, running, _)| name == &session.name && *running)
        {
            return Err(CliError::CommandFailed(format!(
                "session '{}' is already running",
                session.name
            )));
        }
    }
    Ok(())
}

async fn start_env_internal(
    env_name: &str,
    env_def: &EnvDef,
    config_base_dir: Option<&Path>,
    started_sessions: &mut Vec<String>,
) -> Result<(), CliError> {
    for session in &env_def.sessions {
        let response = send_action(
            &session.name,
            Action::Load {
                libpath: session.lib.clone(),
                env_tag: Some(env_name.to_string()),
                init: env_session_init_entries(session)?,
            },
        )
        .await?;
        if !response.success {
            return Err(CliError::CommandFailed(response_error(&response)));
        }
        started_sessions.push(session.name.clone());
    }

    for (bus_name, bus) in &env_def.can {
        apply_env_bus_wiring(bus_name, bus, config_base_dir).await?;
    }
    for (channel_name, channel) in &env_def.shared {
        apply_env_shared_wiring(env_name, channel_name, channel).await?;
    }
    Ok(())
}

async fn apply_env_bus_wiring(
    bus_name: &str,
    bus: &EnvCanBus,
    config_base_dir: Option<&Path>,
) -> Result<(), CliError> {
    for member in &bus.members {
        let (session_name, member_bus_name) = parse_env_member(member, bus_name)?;
        send_action_success(
            &session_name,
            Action::CanAttach {
                bus_name: member_bus_name.clone(),
                vcan_iface: bus.vcan.clone(),
            },
        )
        .await?;
        if let Some(dbc_path) = &bus.dbc {
            let resolved_path = resolve_config_relative_path(dbc_path, config_base_dir)?;
            send_action_success(
                &session_name,
                Action::CanLoadDbc {
                    bus_name: member_bus_name,
                    path: resolved_path,
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn resolve_config_relative_path(
    raw_path: &str,
    config_base_dir: Option<&Path>,
) -> Result<String, CliError> {
    let path = Path::new(raw_path);
    let candidate: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base_dir) = config_base_dir {
        base_dir.join(path)
    } else {
        std::env::current_dir()
            .map_err(|e| {
                CliError::CommandFailed(format!(
                    "failed to determine current working directory while resolving DBC path '{raw_path}': {e}"
                ))
            })?
            .join(path)
    };
    let canonical = std::fs::canonicalize(&candidate).map_err(|e| {
        CliError::CommandFailed(format!(
            "failed to resolve DBC path '{raw_path}' to an absolute path (candidate '{}'): {e}",
            candidate.display()
        ))
    })?;
    Ok(canonical.to_string_lossy().into_owned())
}

async fn apply_env_shared_wiring(
    env_name: &str,
    channel_name: &str,
    channel: &EnvSharedChannel,
) -> Result<(), CliError> {
    let shared_root = lifecycle::session_root().join("shared");
    std::fs::create_dir_all(&shared_root).map_err(|e| {
        CliError::CommandFailed(format!(
            "failed to create shared region root '{}': {e}",
            shared_root.display()
        ))
    })?;
    let region_path = shared_root.join(format!("{env_name}__{channel_name}.bin"));
    let region_path_str = region_path.display().to_string();

    if !channel.members.iter().any(|member| {
        parse_env_member(member, channel_name)
            .map(|(session, _)| session == channel.writer)
            .unwrap_or(false)
    }) {
        return Err(CliError::CommandFailed(format!(
            "shared channel '{channel_name}' writer '{}' is not listed in members",
            channel.writer
        )));
    }

    for member in &channel.members {
        let (session_name, member_channel_name) = parse_env_member(member, channel_name)?;
        send_action_success(
            &session_name,
            Action::SharedAttach {
                channel_name: member_channel_name,
                path: region_path_str.clone(),
                writer: session_name == channel.writer,
                writer_session: channel.writer.clone(),
            },
        )
        .await?;
    }
    Ok(())
}

fn parse_env_member(member: &str, default_bus_name: &str) -> Result<(String, String), CliError> {
    if let Some((session_name, bus_name)) = member.split_once(':') {
        if session_name.is_empty() || bus_name.is_empty() {
            return Err(CliError::CommandFailed(format!(
                "invalid env bus member '{member}'"
            )));
        }
        return Ok((session_name.to_string(), bus_name.to_string()));
    }
    if member.is_empty() {
        return Err(CliError::CommandFailed("empty env bus member".to_string()));
    }
    Ok((member.to_string(), default_bus_name.to_string()))
}

async fn rollback_started_sessions(started_sessions: &[String]) {
    for session_name in started_sessions {
        if send_action_success(session_name, Action::Close)
            .await
            .is_err()
            && let Some(pid) = lifecycle::read_pid(session_name)
        {
            let _ = lifecycle::kill_pid(pid);
        }
    }
}

fn env_session_init_entries(session: &EnvSession) -> Result<Vec<InitEntry>, CliError> {
    session
        .init
        .iter()
        .map(|(key, value)| {
            Ok(InitEntry {
                key: key.clone(),
                value: toml_value_to_signal_value(value)
                    .map_err(|e| CliError::CommandFailed(e.to_string()))?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::resolve_config_relative_path;
    use crate::cli::error::CliError;
    use std::path::Path;

    #[test]
    fn resolve_config_relative_path_joins_relative_to_config_dir() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let config_dir = temp.path().join("cfg");
        let dbc_dir = config_dir.join("dbc");
        let dbc = dbc_dir.join("internal.dbc");
        std::fs::create_dir_all(&dbc_dir).expect("dbc dir should be creatable");
        std::fs::write(&dbc, "VERSION \"\"").expect("dbc file should be writable");

        let resolved = resolve_config_relative_path("dbc/internal.dbc", Some(config_dir.as_path()))
            .expect("relative DBC path should resolve");
        let expected = std::fs::canonicalize(&dbc)
            .expect("dbc should canonicalize")
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_config_relative_path_keeps_absolute_paths() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let absolute = temp.path().join("internal.dbc");
        std::fs::write(&absolute, "VERSION \"\"").expect("dbc file should be writable");
        let resolved = resolve_config_relative_path(
            &absolute.to_string_lossy(),
            Some(Path::new("/tmp/unused")),
        )
        .expect("absolute path should resolve");
        let expected = std::fs::canonicalize(&absolute)
            .expect("absolute path should canonicalize")
            .to_string_lossy()
            .into_owned();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_config_relative_path_rejects_missing_file() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let err = resolve_config_relative_path("missing.dbc", Some(temp.path()))
            .expect_err("missing DBC should fail early");
        let CliError::CommandFailed(message) = err else {
            panic!("expected command failure");
        };
        assert!(
            message.contains("failed to resolve DBC path"),
            "unexpected error: {message}"
        );
    }
}
