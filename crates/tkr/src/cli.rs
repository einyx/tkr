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
        /// Show all commands instead of top 10
        #[arg(long)]
        breakdown: bool,
        /// Sort order: savings (default), in, runs, ratio
        #[arg(long, default_value = "savings")]
        sort: String,
        /// Render plain output (no colors / box characters)
        #[arg(long)]
        plain: bool,
    },
    /// Analyze session history for missed savings
    Discover,
    /// Rewrite a shell command to use tkr (used by hooks).
    /// Exit 0 + stdout: rewrite found. Exit 1: no rewrite available.
    Rewrite {
        /// The full shell command line to rewrite.
        command: String,
    },
    /// Hook integration. Reads JSON from stdin, emits JSON on stdout.
    Hook {
        #[command(subcommand)]
        target: HookTarget,
    },
    /// Print the version.
    Version,
    /// Erase the analytics database (savings history reset).
    CleanStats {
        /// Don't prompt — just delete.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum HookTarget {
    /// Claude Code PreToolUse Bash hook. Reads `{"tool_input":{"command":...}}`,
    /// emits the rewritten command with `permissionDecision: allow`.
    Claude,
}
