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

    /// Internal helper mode for detached instance bootstrap
    #[arg(long, global = true, hide = true)]
    pub bootstrap_instance: bool,

    /// Internal env daemon mode
    #[arg(long, global = true, hide = true)]
    pub env_daemon: bool,

    /// JSON output mode
    #[arg(long, global = true, env = "AGENT_SIM_JSON", default_value_t = false)]
    pub json: bool,

    /// Named instance
    #[arg(
        long,
        global = true,
        env = "AGENT_SIM_INSTANCE",
        default_value = "default"
    )]
    pub instance: String,

    /// Internal daemon startup DLL path
    #[arg(long, global = true, hide = true)]
    pub libpath: Option<String>,

    /// Internal daemon startup load spec path
    #[arg(long, global = true, hide = true)]
    pub load_spec_path: Option<String>,

    /// Internal env tag metadata for daemon startup
    #[arg(long, global = true, hide = true)]
    pub env_tag: Option<String>,

    /// Internal env daemon startup env name
    #[arg(long, global = true, hide = true)]
    pub env_name: Option<String>,

    /// Internal env daemon startup env spec path
    #[arg(long, global = true, hide = true)]
    pub env_spec_path: Option<String>,

    /// Config file path
    #[arg(long, global = true, env = "AGENT_SIM_CONFIG")]
    pub config: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Load(LoadArgs),
    Info,
    Signals,
    Can(CanArgs),
    Shared(SharedArgs),
    Reset,
    Get(GetArgs),
    Set(SetArgs),
    Watch(WatchArgs),
    Run(RunArgs),
    Close(CloseArgs),
    Env(EnvArgs),
    Instance(InstanceArgs),
    Time(TimeArgs),
}

#[derive(Debug, Args)]
pub struct LoadArgs {
    pub libpath: Option<String>,
    #[arg(long = "flash")]
    pub flash: Vec<String>,
}

#[derive(Debug, Args)]
pub struct CloseArgs {
    #[arg(long, default_value_t = false)]
    pub all: bool,
    #[arg(long)]
    pub env: Option<String>,
}

#[derive(Debug, Args)]
pub struct EnvArgs {
    #[command(subcommand)]
    pub command: EnvCommand,
}

#[derive(Debug, Subcommand)]
pub enum EnvCommand {
    Start {
        name: String,
    },
    Status {
        name: String,
    },
    Reset {
        name: String,
    },
    Time {
        name: String,
        #[command(subcommand)]
        command: TimeCommand,
    },
    Can {
        name: String,
        #[command(subcommand)]
        command: EnvCanCommand,
    },
}

#[derive(Debug, Args)]
pub struct InstanceArgs {
    #[command(subcommand)]
    pub command: Option<InstanceCommand>,
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
pub enum EnvCanCommand {
    Buses,
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
    Inspect {
        bus: String,
    },
    Schedule {
        #[command(subcommand)]
        command: EnvCanScheduleCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum EnvCanScheduleCommand {
    Add {
        bus: String,
        arb_id: String,
        data_hex: String,
        every: String,
        #[arg(long)]
        job_id: Option<String>,
        #[arg(long)]
        flags: Option<u8>,
    },
    Update {
        job_id: String,
        arb_id: String,
        data_hex: String,
        every: String,
        #[arg(long)]
        flags: Option<u8>,
    },
    Remove {
        job_id: String,
    },
    Stop {
        job_id: String,
    },
    List {
        bus: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct SharedArgs {
    #[command(subcommand)]
    pub command: SharedCommand,
}

#[derive(Debug, Subcommand)]
pub enum SharedCommand {
    List,
    Get { channel: String },
}

#[derive(Debug, Subcommand)]
pub enum InstanceCommand {
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
