use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "agent-audit",
    about = "Run the local smart contract audit pipeline scaffold."
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    InitRun(InitRunArgs),
    FetchSource(RunIdArgs),
    FetchBytecode(RunIdArgs),
    RunDependency(RunIdArgs),
    PrepareHeimdall(RunIdArgs),
    PrepareSlither(RunIdArgs),
    PrepareTooling(RunIdArgs),
    AggregateMaterials(RunIdArgs),
    SyncRun(RunIdArgs),
}

#[derive(Args)]
pub(crate) struct InitRunArgs {
    #[arg(long)]
    pub(crate) address: String,
    #[arg(long)]
    pub(crate) chain: Option<String>,
    #[arg(long = "source-kind")]
    pub(crate) source_kind: Option<String>,
    #[arg(long = "closed-source")]
    pub(crate) closed_source: bool,
}

#[derive(Args)]
pub(crate) struct RunIdArgs {
    #[arg(long = "run-id")]
    pub(crate) run_id: String,
}
