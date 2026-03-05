use crate::cli::args::{CanCommand, CliArgs, Command, SessionCommand, SetArgs, TimeCommand};
use crate::cli::error::CliError;
use crate::protocol::{Action, Request};
use std::collections::BTreeMap;
use uuid::Uuid;

pub fn to_request(args: &CliArgs) -> Result<Request, CliError> {
    let command = args.command.as_ref().ok_or(CliError::MissingCommand)?;
    let action = match command {
        Command::Load { libpath } => Action::Load {
            libpath: libpath.clone(),
        },
        Command::Info => Action::Info,
        Command::Signals => Action::Signals,
        Command::Can(can) => match &can.command {
            CanCommand::Buses => Action::CanBuses,
            CanCommand::Attach { bus, vcan_iface } => Action::CanAttach {
                bus_name: bus.clone(),
                vcan_iface: vcan_iface.clone(),
            },
            CanCommand::Detach { bus } => Action::CanDetach {
                bus_name: bus.clone(),
            },
            CanCommand::Send {
                bus,
                arb_id,
                data_hex,
                flags,
            } => Action::CanSend {
                bus_name: bus.clone(),
                arb_id: parse_arb_id(arb_id)?,
                data_hex: data_hex.clone(),
                flags: *flags,
            },
        },
        Command::Reset => Action::Reset,
        Command::Get(get) => Action::Get {
            selectors: get.selectors.clone(),
        },
        Command::Set(set) => Action::Set {
            writes: parse_set_entries(set)?,
        },
        Command::Close => Action::Close,
        Command::Session(session) => match session.command {
            Some(SessionCommand::List) => Action::SessionList,
            None => Action::SessionStatus,
        },
        Command::Time(time) => match &time.command {
            TimeCommand::Start => Action::TimeStart,
            TimeCommand::Pause => Action::TimePause,
            TimeCommand::Step { duration } => Action::TimeStep {
                duration: duration.clone(),
            },
            TimeCommand::Speed { multiplier } => Action::TimeSpeed {
                multiplier: *multiplier,
            },
            TimeCommand::Status => Action::TimeStatus,
        },
        Command::Watch(_) | Command::Run(_) => {
            return Err(CliError::CommandFailed(
                "watch/run are handled by the CLI executor".to_string(),
            ));
        }
    };
    Ok(Request {
        id: Uuid::new_v4(),
        action,
    })
}

fn parse_arb_id(value: &str) -> Result<u32, CliError> {
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
}
