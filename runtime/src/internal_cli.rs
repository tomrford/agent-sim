use clap::{Parser, Subcommand};
use std::ffi::{OsStr, OsString};

#[derive(Debug, Parser)]
#[command(name = "agent-sim", hide = true)]
struct InternalCli {
    #[command(subcommand)]
    command: InternalCommand,
}

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    InstanceDaemon {
        #[arg(long)]
        instance: String,
        #[arg(long)]
        load_spec_path: String,
    },
    BootstrapInstance {
        #[arg(long)]
        instance: String,
        #[arg(long)]
        load_spec_path: String,
    },
    EnvDaemon {
        #[arg(long)]
        env_spec_path: String,
    },
}

pub fn parse_from_env_if_internal() -> Option<InternalCommand> {
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    if raw_args
        .get(1)
        .is_none_or(|arg| arg != OsStr::new("__internal"))
    {
        return None;
    }

    let args = std::iter::once(raw_args[0].clone())
        .chain(raw_args.into_iter().skip(2))
        .collect::<Vec<OsString>>();
    Some(InternalCli::parse_from(args).command)
}
