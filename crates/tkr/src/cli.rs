use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "tkr", about = "Token-optimized CLI proxy")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Hard token-output budget; lines past this point are elided with a
    /// `(... N more lines elided)` marker. Equivalent to `TKR_MAX_TOKENS=N`.
    #[arg(long)]
    pub max_tokens: Option<u64>,

    /// If the entire output is a JSON document, re-emit it compact (no
    /// whitespace). Buffers stdout, parses, reserializes. Falls back to the
    /// unchanged buffered text when the output isn't JSON. Big win on
    /// `kubectl get -o json`, `aws describe-*`, etc.
    /// Equivalent to `TKR_COMPACT_JSON=1`.
    #[arg(long)]
    pub compact_json: bool,

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
    /// Suggest concrete filter improvements based on your analytics
    Suggest,
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
    /// Install the tkr Claude Code Bash hook into ~/.claude/settings.json.
    Install,
    /// Benchmark how much tkr would save on a given command.
    /// Runs raw vs filtered, compares chars/tokens, prints ratio.
    Bench {
        /// The command + args to benchmark (e.g. `tkr bench cargo check`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Run an agent from a TOML manifest
    Agent {
        #[command(subcommand)]
        cmd: AgentCmd,
    },
    /// Self-update: download and install the latest release from GitHub.
    Update {
        /// Only check, do not install.
        #[arg(long)]
        check: bool,
        /// Reinstall even if already on the latest version.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum HookTarget {
    /// Claude Code PreToolUse Bash hook. Reads `{"tool_input":{"command":...}}`,
    /// emits the rewritten command with `permissionDecision: allow`.
    Claude,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Execute one agent run
    Run {
        /// Path to a TOML manifest
        manifest: std::path::PathBuf,
    },
}
