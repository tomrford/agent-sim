use super::response_error;
use crate::cli::args::{
    CliArgs, EnvArgs, EnvCanCommand, EnvCanScheduleCommand, EnvCommand, TimeCommand,
};
use crate::cli::error::CliError;
use crate::config::load_config;
use crate::config::recipe::EnvDef;
use crate::connection::send_env_request;
use crate::daemon::lifecycle;
use crate::envd::lifecycle::bootstrap_env_daemon;
use crate::envd::spec::{
    EnvCanBusMemberSpec, EnvCanBusSpec, EnvInstanceSpec, EnvSharedChannelMemberSpec,
    EnvSharedChannelSpec, EnvSpec,
};
use crate::load::resolve::{canonicalize_runtime_path, resolve_env_load_specs};
use crate::protocol::{Action, Request};
use std::path::Path;
use std::process::ExitCode;
use uuid::Uuid;

pub(crate) async fn run_env_command(args: &CliArgs, env: &EnvArgs) -> Result<ExitCode, CliError> {
    match &env.command {
        EnvCommand::Start { name } => run_env_start(args, name).await,
        EnvCommand::Status { name } => {
            run_env_action(args, name, Action::EnvStatus { env: name.clone() }).await
        }
        EnvCommand::Reset { name } => {
            run_env_action(args, name, Action::EnvReset { env: name.clone() }).await
        }
        EnvCommand::Time { name, command } => {
            let action = match command {
                TimeCommand::Start => Action::EnvTimeStart { env: name.clone() },
                TimeCommand::Pause => Action::EnvTimePause { env: name.clone() },
                TimeCommand::Step { duration } => Action::EnvTimeStep {
                    env: name.clone(),
                    duration: duration.clone(),
                },
                TimeCommand::Speed { multiplier } => Action::EnvTimeSpeed {
                    env: name.clone(),
                    multiplier: *multiplier,
                },
                TimeCommand::Status => Action::EnvTimeStatus { env: name.clone() },
            };
            run_env_action(args, name, action).await
        }
        EnvCommand::Can { name, command } => {
            let action = match command {
                EnvCanCommand::Buses => Action::EnvCanBuses { env: name.clone() },
                EnvCanCommand::LoadDbc { bus, path } => Action::EnvCanLoadDbc {
                    env: name.clone(),
                    bus_name: bus.clone(),
                    path: canonicalize_runtime_path(path, None, "DBC")
                        .map_err(CliError::CommandFailed)?,
                },
                EnvCanCommand::Send {
                    bus,
                    arb_id,
                    data_hex,
                    flags,
                } => Action::EnvCanSend {
                    env: name.clone(),
                    bus_name: bus.clone(),
                    arb_id: super::commands::parse_arb_id(arb_id)?,
                    data_hex: data_hex.clone(),
                    flags: *flags,
                },
                EnvCanCommand::Inspect { bus } => Action::EnvCanInspect {
                    env: name.clone(),
                    bus_name: bus.clone(),
                },
                EnvCanCommand::Schedule { command } => match command {
                    EnvCanScheduleCommand::Add {
                        bus,
                        arb_id,
                        data_hex,
                        every,
                        job_id,
                        flags,
                    } => Action::EnvCanScheduleAdd {
                        env: name.clone(),
                        bus_name: bus.clone(),
                        job_id: job_id.clone(),
                        arb_id: super::commands::parse_arb_id(arb_id)?,
                        data_hex: data_hex.clone(),
                        every: every.clone(),
                        flags: *flags,
                    },
                    EnvCanScheduleCommand::Update {
                        job_id,
                        arb_id,
                        data_hex,
                        every,
                        flags,
                    } => Action::EnvCanScheduleUpdate {
                        env: name.clone(),
                        job_id: job_id.clone(),
                        arb_id: super::commands::parse_arb_id(arb_id)?,
                        data_hex: data_hex.clone(),
                        every: every.clone(),
                        flags: *flags,
                    },
                    EnvCanScheduleCommand::Remove { job_id } => Action::EnvCanScheduleRemove {
                        env: name.clone(),
                        job_id: job_id.clone(),
                    },
                    EnvCanScheduleCommand::Stop { job_id } => Action::EnvCanScheduleStop {
                        env: name.clone(),
                        job_id: job_id.clone(),
                    },
                    EnvCanScheduleCommand::Start { job_id } => Action::EnvCanScheduleStart {
                        env: name.clone(),
                        job_id: job_id.clone(),
                    },
                    EnvCanScheduleCommand::List { bus } => Action::EnvCanScheduleList {
                        env: name.clone(),
                        bus_name: bus.clone(),
                    },
                },
            };
            run_env_action(args, name, action).await
        }
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
    let env_spec = build_env_spec(
        env_name,
        &env_def,
        &config.file.device,
        config_base_dir.as_deref(),
    )?;
    bootstrap_env_daemon(&env_spec)
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    run_env_action(
        args,
        env_name,
        Action::EnvStatus {
            env: env_name.to_string(),
        },
    )
    .await
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
    for session in &env_def.instances {
        if sessions
            .iter()
            .any(|(name, _, running, _)| name == &session.name && *running)
        {
            return Err(CliError::CommandFailed(format!(
                "instance '{}' is already running",
                session.name
            )));
        }
    }
    Ok(())
}

fn build_env_spec(
    env_name: &str,
    env_def: &EnvDef,
    devices: &std::collections::BTreeMap<String, crate::config::recipe::DeviceDef>,
    config_base_dir: Option<&Path>,
) -> Result<EnvSpec, CliError> {
    let load_specs = resolve_env_load_specs(env_name, &env_def.instances, devices, config_base_dir)
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    let instances = load_specs
        .into_iter()
        .map(|(name, load_spec)| EnvInstanceSpec { name, load_spec })
        .collect::<Vec<_>>();

    let mut can_buses = Vec::with_capacity(env_def.can.len());
    for (bus_name, bus) in &env_def.can {
        can_buses.push(EnvCanBusSpec {
            name: bus_name.clone(),
            vcan_iface: bus.vcan.clone(),
            dbc_path: bus
                .dbc
                .as_ref()
                .map(|path| resolve_config_relative_path(path, config_base_dir, "DBC"))
                .transpose()?,
            members: bus
                .members
                .iter()
                .map(|member| {
                    let (instance_name, member_bus_name) = parse_env_member(member, bus_name)?;
                    Ok(EnvCanBusMemberSpec {
                        instance_name,
                        bus_name: member_bus_name,
                    })
                })
                .collect::<Result<Vec<_>, CliError>>()?,
        });
    }

    let mut shared_channels = Vec::with_capacity(env_def.shared.len());
    for (channel_name, channel) in &env_def.shared {
        shared_channels.push(EnvSharedChannelSpec {
            name: channel_name.clone(),
            writer_instance: channel.writer.clone(),
            members: channel
                .members
                .iter()
                .map(|member| {
                    let (instance_name, member_channel_name) =
                        parse_env_member(member, channel_name)?;
                    Ok(EnvSharedChannelMemberSpec {
                        instance_name,
                        channel_name: member_channel_name,
                    })
                })
                .collect::<Result<Vec<_>, CliError>>()?,
        });
    }

    Ok(EnvSpec {
        name: env_name.to_string(),
        instances,
        can_buses,
        shared_channels,
    })
}

fn resolve_config_relative_path(
    raw_path: &str,
    config_base_dir: Option<&Path>,
    kind: &str,
) -> Result<String, CliError> {
    canonicalize_runtime_path(raw_path, config_base_dir, kind).map_err(CliError::CommandFailed)
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

async fn run_env_action(
    args: &CliArgs,
    env_name: &str,
    action: Action,
) -> Result<ExitCode, CliError> {
    let request = Request {
        id: Uuid::new_v4(),
        action,
    };
    let response = send_env_request(env_name, &request)
        .await
        .map_err(|err| CliError::CommandFailed(err.to_string()))?;
    crate::cli::output::print_response(&response, args.json);
    if response.success {
        Ok(ExitCode::SUCCESS)
    } else {
        Err(CliError::CommandFailed(response_error(&response)))
    }
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

        let resolved =
            resolve_config_relative_path("dbc/internal.dbc", Some(config_dir.as_path()), "DBC")
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
            "DBC",
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
        let err = resolve_config_relative_path("missing.dbc", Some(temp.path()), "DBC")
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
