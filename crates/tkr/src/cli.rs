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
    /// Analyze shell history for missed savings
    Discover {
        /// Optional path to a shell history file
        #[arg(long)]
        history: Option<std::path::PathBuf>,
        /// Max number of recent history lines to scan
        #[arg(long, default_value_t = 50000)]
        limit: usize,
    },
    /// Suggest concrete filter improvements based on your analytics
    Suggest,
    /// Explain what tkr filtered in the latest persisted agent run
    Explain {
        /// Optional run record JSON path (defaults to most recent in ~/.tkr/runs)
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
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
    /// Install the tkr hook into AI coding tools.
    /// Auto-detects installed tools when no flag is given.
    Install {
        /// Install only into Claude Code (~/.claude/settings.json)
        #[arg(long)]
        claude: bool,
        /// Install only into Codex CLI (~/.codex/config.toml)
        #[arg(long)]
        codex: bool,
    },
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
    /// Encrypted vault (plugin storage under ~/.tkr/vault/)
    Vault {
        #[command(subcommand)]
        cmd: Option<VaultCmd>,
    },
    /// Administrative maintenance (dangerous operations)
    Admin {
        #[command(subcommand)]
        cmd: AdminCmd,
    },
}

/// Subcommands for `tkr vault`. When omitted, defaults to `status`.
#[derive(Subcommand, Debug, Clone)]
pub enum VaultCmd {
    /// Print vault seal state and paths
    Status,
    /// Promote to fully-unsealed (Private + Secret storage classes)
    Unseal,
    /// Seal the vault (Secret-class data inaccessible until unseal)
    Seal,
    /// Initialize vault dir and persist master key
    Init,
    /// Rotate master key and re-encrypt vault entries
    Rotate,
    /// Export vault to a .tar.gz bundle (optional output path)
    Export {
        /// Output path (default: sibling of vault dir with .tar.gz extension)
        path: Option<std::path::PathBuf>,
    },
    /// Import a vault bundle from `vault export`
    Import {
        /// Path to bundle.tar.gz
        bundle: std::path::PathBuf,
    },
    /// Print or verify the vault audit log
    Audit {
        /// Verify HMAC chain integrity
        #[arg(long)]
        verify: bool,
        /// Show only the last N entries
        #[arg(long, value_name = "N")]
        last: Option<usize>,
    },
}

#[derive(Subcommand, Debug)]
pub enum AdminCmd {
    /// Delete all vault entries owned by a plugin
    Reset {
        /// Plugin name (manifest `name`)
        #[arg(long)]
        plugin: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum HookTarget {
    /// Claude Code PreToolUse Bash hook. Reads `{"tool_input":{"command":...}}`,
    /// emits the rewritten command with `permissionDecision: allow`.
    Claude,
    /// Same JSON response as `claude`; also accepts a top-level `"command"` field
    /// for shells / IDE wrappers that do not nest under `tool_input`.
    Universal,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Execute one agent run
    Run {
        /// Path to a TOML manifest
        manifest: std::path::PathBuf,
    },
}
