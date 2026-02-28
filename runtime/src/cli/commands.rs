use crate::cli::args::{CliArgs, Command, InstanceCommand, SessionCommand, SetArgs, TimeCommand};
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
        Command::Unload => Action::Unload,
        Command::Info => Action::Info,
        Command::Signals => Action::Signals,
        Command::Get(get) => Action::Get {
            selectors: get.selectors.clone(),
            instance: args.instance,
        },
        Command::Set(set) => Action::Set {
            writes: parse_set_entries(set)?,
            instance: args.instance,
        },
        Command::Watch(watch) => Action::Watch {
            selector: watch.selector.clone(),
            interval_ms: watch.interval_ms,
            samples: watch.samples,
            instance: args.instance,
        },
        Command::Run(run) => Action::RunRecipe {
            recipe: run.recipe_name.clone(),
            dry_run: run.dry_run,
            config: args.config.clone(),
        },
        Command::Close => Action::Close,
        Command::Session(session) => match session.command {
            Some(SessionCommand::List) => Action::SessionList,
            None => Action::SessionStatus,
        },
        Command::Instance(instance) => match &instance.command {
            InstanceCommand::New => Action::InstanceNew,
            InstanceCommand::List => Action::InstanceList,
            InstanceCommand::Select { index } => Action::InstanceSelect { index: *index },
            InstanceCommand::Reset { index } => Action::InstanceReset { index: *index },
            InstanceCommand::Free { index } => Action::InstanceFree { index: *index },
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
    };
    Ok(Request {
        id: Uuid::new_v4(),
        action,
    })
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
}
