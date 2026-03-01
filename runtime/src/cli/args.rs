use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "agent-sim",
    version,
    about = "Stateful firmware simulation runtime CLI"
)]
pub struct CliArgs {
    /// Internal daemon mode
    #[arg(long, global = true, hide = true)]
    pub daemon: bool,

    /// JSON output mode
    #[arg(long, global = true, env = "AGENT_SIM_JSON", default_value_t = false)]
    pub json: bool,

    /// Named session
    #[arg(
        long,
        global = true,
        env = "AGENT_SIM_SESSION",
        default_value = "default"
    )]
    pub session: String,

    /// Override active instance for this command
    #[arg(long, global = true, env = "AGENT_SIM_INSTANCE")]
    pub instance: Option<u32>,

    /// Config file path
    #[arg(long, global = true, env = "AGENT_SIM_CONFIG")]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Load { libpath: String },
    Unload,
    Info,
    Signals,
    Get(GetArgs),
    Set(SetArgs),
    Watch(WatchArgs),
    Run(RunArgs),
    Close,
    Session(SessionArgs),
    Instance(InstanceArgs),
    Time(TimeArgs),
}

#[derive(Debug, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: Option<SessionCommand>,
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List,
}

#[derive(Debug, Args)]
pub struct InstanceArgs {
    #[command(subcommand)]
    pub command: InstanceCommand,
}

#[derive(Debug, Subcommand)]
pub enum InstanceCommand {
    New,
    List,
    Select { index: u32 },
    Reset { index: Option<u32> },
    Free { index: u32 },
}

#[derive(Debug, Args)]
pub struct TimeArgs {
    #[command(subcommand)]
    pub command: TimeCommand,
}

#[derive(Debug, Subcommand)]
pub enum TimeCommand {
    Start,
    Pause,
    Step { duration: String },
    Speed { multiplier: Option<f64> },
    Status,
}

#[derive(Debug, Args)]
pub struct GetArgs {
    #[arg(required = true)]
    pub selectors: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SetArgs {
    /// Either "<signal> <value>" or repeated "<signal>=<value>" pairs
    #[arg(required = true)]
    pub entries: Vec<String>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    pub selector: String,
    #[arg(default_value_t = 200)]
    pub interval_ms: u64,
    #[arg(long)]
    pub samples: Option<u32>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    pub recipe_name: String,
    #[arg(long)]
    pub dry_run: bool,
}
