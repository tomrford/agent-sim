use crate::cli::args::{
    CanCommand, CliArgs, Command, InstanceCommand, SetArgs, SharedCommand, TimeCommand,
    TraceCommand,
};
use crate::cli::error::CliError;
use crate::protocol::{InstanceAction, Request, RequestAction};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn to_request(args: &CliArgs) -> Result<Request, CliError> {
    let command = args.command.as_ref().ok_or(CliError::MissingCommand)?;
    let action = match command {
        Command::Info => InstanceAction::Info,
        Command::Signals => InstanceAction::Signals,
        Command::Shared(shared) => match &shared.command {
            SharedCommand::List => InstanceAction::SharedList,
            SharedCommand::Get { channel } => InstanceAction::SharedGet {
                channel_name: parse_shared_channel_selector(channel)?,
            },
        },
        Command::Can(can) => match &can.command {
            CanCommand::Buses => InstanceAction::CanBuses,
            CanCommand::Attach { bus, vcan_iface } => InstanceAction::CanAttach {
                bus_name: bus.clone(),
                vcan_iface: vcan_iface.clone(),
            },
            CanCommand::Detach { bus } => InstanceAction::CanDetach {
                bus_name: bus.clone(),
            },
            CanCommand::LoadDbc { bus, path } => InstanceAction::CanLoadDbc {
                bus_name: bus.clone(),
                path: canonicalize_cli_path(path)?,
            },
            CanCommand::Send {
                bus,
                arb_id,
                data_hex,
                flags,
            } => InstanceAction::CanSend {
                bus_name: bus.clone(),
                arb_id: parse_arb_id(arb_id)?,
                data_hex: data_hex.clone(),
                flags: *flags,
            },
        },
        Command::Reset => InstanceAction::Reset,
        Command::Get(get) => InstanceAction::Get {
            selectors: get.selectors.clone(),
        },
        Command::Set(set) => InstanceAction::Set {
            writes: parse_set_entries(set)?,
        },
        Command::Trace(trace) => match &trace.command {
            TraceCommand::Start { path, period } => InstanceAction::TraceStart {
                path: absolutize_cli_path(path, "trace output").map_err(CliError::CommandFailed)?,
                period: period.clone(),
            },
            TraceCommand::Stop => InstanceAction::TraceStop,
            TraceCommand::Clear => InstanceAction::TraceClear,
            TraceCommand::Status => InstanceAction::TraceStatus,
        },
        Command::Close(close) if !close.all && close.env.is_none() => InstanceAction::Close,
        Command::Instance(instance) => match instance.command {
            Some(InstanceCommand::List) => InstanceAction::InstanceList,
            None => InstanceAction::InstanceStatus,
        },
        Command::Time(time) => match &time.command {
            TimeCommand::Start => InstanceAction::TimeStart,
            TimeCommand::Pause => InstanceAction::TimePause,
            TimeCommand::Step { duration } => InstanceAction::TimeStep {
                duration: duration.clone(),
            },
            TimeCommand::Speed { multiplier } => InstanceAction::TimeSpeed {
                multiplier: *multiplier,
            },
            TimeCommand::Status => InstanceAction::TimeStatus,
        },
        Command::Load(_) | Command::Run(_) | Command::Env(_) | Command::Close(_) => {
            return Err(CliError::CommandFailed(
                "command is handled by the CLI executor".to_string(),
            ));
        }
    };
    Ok(Request {
        id: Uuid::new_v4(),
        action: RequestAction::Instance(action),
    })
}

fn canonicalize_cli_path(raw_path: &str) -> Result<String, CliError> {
    let path = Path::new(raw_path);
    let candidate: PathBuf = if path.is_absolute() {
        path.to_path_buf()
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

pub(crate) fn absolutize_cli_path(raw_path: &str, kind: &str) -> Result<String, String> {
    let path = Path::new(raw_path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| {
                format!(
                    "failed to determine current working directory while resolving {kind} path '{raw_path}': {err}"
                )
            })?
            .join(path)
    };
    Ok(absolute.to_string_lossy().into_owned())
}

pub(crate) fn parse_arb_id(value: &str) -> Result<u32, CliError> {
    let trimmed = value.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16)
            .map_err(|_| CliError::CommandFailed(format!("invalid arbitration id '{value}'")))
    } else {
        trimmed
            .parse::<u32>()
            .map_err(|_| CliError::CommandFailed(format!("invalid arbitration id '{value}'")))
    }
}

fn parse_shared_channel_selector(value: &str) -> Result<String, CliError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CliError::CommandFailed(
            "shared get requires a channel selector".to_string(),
        ));
    }
    if let Some(name) = trimmed.strip_suffix(".*") {
        if name.is_empty() {
            return Err(CliError::CommandFailed(format!(
                "invalid shared selector '{value}'"
            )));
        }
        return Ok(name.to_string());
    }
    Ok(trimmed.to_string())
}

fn parse_set_entries(args: &SetArgs) -> Result<BTreeMap<String, String>, CliError> {
    if args.entries.len() == 2 && !args.entries[0].contains('=') && !args.entries[1].contains('=') {
        let mut map = BTreeMap::new();
        map.insert(args.entries[0].clone(), args.entries[1].clone());
        return Ok(map);
    }

    let mut out = BTreeMap::new();
    for entry in &args.entries {
        let Some((k, v)) = entry.split_once('=') else {
            return Err(CliError::InvalidSetSyntax);
        };
        if k.trim().is_empty() {
            return Err(CliError::InvalidSetSyntax);
        }
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    if out.is_empty() {
        return Err(CliError::InvalidSetSyntax);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{CanArgs, CliArgs, Command, LoadArgs};
    use clap::Parser;

    #[test]
    fn set_parser_accepts_single_pair() {
        let set = SetArgs {
            entries: vec!["sig".to_string(), "1".to_string()],
        };
        let writes = parse_set_entries(&set).expect("single pair set syntax should parse");
        assert_eq!(writes.get("sig"), Some(&"1".to_string()));
    }

    #[test]
    fn set_parser_accepts_equals_pairs() {
        let set = SetArgs {
            entries: vec!["a=1".to_string(), "b=true".to_string()],
        };
        let writes = parse_set_entries(&set).expect("equals-pairs set syntax should parse");
        assert_eq!(writes.len(), 2);
    }

    #[test]
    fn set_parser_rejects_mixed_syntax() {
        let set = SetArgs {
            entries: vec!["a=1".to_string(), "b".to_string()],
        };
        assert!(parse_set_entries(&set).is_err());
    }

    #[test]
    fn arb_id_parser_accepts_hex_and_decimal() {
        assert_eq!(
            parse_arb_id("0x7FF").expect("hex arb id should parse"),
            0x7FF
        );
        assert_eq!(
            parse_arb_id("2048").expect("decimal arb id should parse"),
            2048
        );
        assert!(parse_arb_id("xyz").is_err());
    }

    #[test]
    fn shared_selector_parser_accepts_wildcard_suffix() {
        assert_eq!(
            parse_shared_channel_selector("sensor_feed.*").expect("shared selector should parse"),
            "sensor_feed"
        );
        assert_eq!(
            parse_shared_channel_selector("sensor_feed").expect("plain selector should parse"),
            "sensor_feed"
        );
        assert!(parse_shared_channel_selector(".*").is_err());
    }

    #[test]
    fn can_load_dbc_request_resolves_relative_path_to_absolute() {
        let cwd = std::env::current_dir().expect("current directory should be readable");
        let dbc = tempfile::Builder::new()
            .prefix("can-load-dbc-")
            .suffix(".dbc")
            .tempfile_in(&cwd)
            .expect("temp dbc should be creatable");
        std::fs::write(dbc.path(), "VERSION \"\"").expect("temp dbc should be writable");
        let relative = dbc
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temp dbc filename should be utf8")
            .to_string();
        let expected = std::fs::canonicalize(dbc.path()).expect("temp dbc should canonicalize");
        let args = CliArgs {
            json: false,
            instance: "default".to_string(),
            config: None,
            command: Some(Command::Can(CanArgs {
                command: CanCommand::LoadDbc {
                    bus: "internal".to_string(),
                    path: relative,
                },
            })),
        };
        let request = to_request(&args).expect("can load-dbc request should build");
        let RequestAction::Instance(InstanceAction::CanLoadDbc { path, .. }) = request.action
        else {
            panic!("expected can load-dbc action");
        };
        assert_eq!(Path::new(&path), expected.as_path());
    }

    #[test]
    fn can_load_dbc_request_rejects_missing_path() {
        let args = CliArgs {
            json: false,
            instance: "default".to_string(),
            config: None,
            command: Some(Command::Can(CanArgs {
                command: CanCommand::LoadDbc {
                    bus: "internal".to_string(),
                    path: "__missing_dbc_for_test__.dbc".to_string(),
                },
            })),
        };
        let err = to_request(&args).expect_err("missing DBC should fail early");
        let CliError::CommandFailed(message) = err else {
            panic!("expected command failure");
        };
        assert!(
            message.contains("failed to resolve DBC path"),
            "unexpected error: {message}"
        );
    }

    #[test]
    fn load_request_is_handled_by_cli_executor() {
        let fake_lib = std::env::temp_dir().join("libsim-test");
        let args = CliArgs {
            json: false,
            instance: "default".to_string(),
            config: None,
            command: Some(Command::Load(LoadArgs {
                libpath: Some(fake_lib.to_string_lossy().into_owned()),
                flash: Vec::new(),
            })),
        };
        let err = to_request(&args).expect_err("load request should be rejected");
        assert!(matches!(err, CliError::CommandFailed(_)));
    }

    #[test]
    fn trace_start_request_absolutizes_output_path() {
        let args =
            CliArgs::try_parse_from(["agent-sim", "trace", "start", "trace-output.csv", "1ms"])
                .expect("trace command should parse");
        let request = to_request(&args).expect("trace start request should build");
        let RequestAction::Instance(InstanceAction::TraceStart { path, period }) = request.action
        else {
            panic!("expected trace start action");
        };
        assert_eq!(period, "1ms");
        assert!(
            Path::new(&path).is_absolute(),
            "trace output path should be absolute, got: {path}"
        );
    }
}
