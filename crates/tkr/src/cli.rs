use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "tkr", about = "Token-optimized CLI proxy")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub passthrough: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Live token savings dashboard
    Watch,
    /// Show token savings analytics
    Gain {
        #[arg(long)]
        breakdown: bool,
    },
    /// Analyze session history for missed savings
    Discover,
}
