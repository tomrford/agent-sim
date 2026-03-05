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

    /// Internal daemon startup DLL path
    #[arg(long, global = true, hide = true)]
    pub libpath: Option<String>,

    /// Config file path
    #[arg(long, global = true, env = "AGENT_SIM_CONFIG")]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Load { libpath: String },
    Info,
    Signals,
    Can(CanArgs),
    Reset,
    Get(GetArgs),
    Set(SetArgs),
    Watch(WatchArgs),
    Run(RunArgs),
    Close,
    Session(SessionArgs),
    Time(TimeArgs),
}

#[derive(Debug, Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: Option<SessionCommand>,
}

#[derive(Debug, Args)]
pub struct CanArgs {
    #[command(subcommand)]
    pub command: CanCommand,
}

#[derive(Debug, Subcommand)]
pub enum CanCommand {
    Buses,
    Attach {
        bus: String,
        vcan_iface: String,
    },
    Detach {
        bus: String,
    },
    LoadDbc {
        bus: String,
        path: String,
    },
    Send {
        bus: String,
        arb_id: String,
        data_hex: String,
        #[arg(long)]
        flags: Option<u8>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SessionCommand {
    List,
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
