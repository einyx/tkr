use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "tkr",
    about = "Token-optimized CLI proxy",
    subcommand_precedence_over_arg = true
)]
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
        /// Install only into Cursor (~/.cursor/rules/tkr.mdc)
        #[arg(long)]
        cursor: bool,
        /// Also install the foundry toolchain (forge/anvil/cast) needed for
        /// tkr-mesh smart-contract development. Idempotent — skips if forge
        /// is already on PATH.
        #[arg(long)]
        with_foundry: bool,
    },
    /// Remove the tkr hook from AI coding tools.
    /// Auto-detects installed tools when no flag is given.
    Uninstall {
        /// Remove only from Claude Code (~/.claude/settings.json)
        #[arg(long)]
        claude: bool,
        /// Remove only from Codex CLI (~/.codex/config.toml)
        #[arg(long)]
        codex: bool,
        /// Remove only from Cursor (~/.cursor/rules/tkr.mdc)
        #[arg(long)]
        cursor: bool,
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
    /// tkr-mesh agent payments (Base / EVM). On-chain settlement runs
    /// against MeshEscrow.sol; off-chain receipts are EIP-712 signatures.
    Pay {
        #[command(subcommand)]
        cmd: PayCmd,
    },
    /// tkr-mesh peer messaging — join a mesh, tail incoming messages,
    /// send DMs to other peers.
    Mesh {
        #[command(subcommand)]
        cmd: MeshCmd,
    },
    /// Run tkr's MCP server on stdio. Exposes tkr_outline_file,
    /// tkr_find_symbol, tkr_grep_summary so AI agents read structured
    /// summaries instead of raw file contents. Registered automatically
    /// by `tkr install`.
    Mcp,
    /// JobBoard — agent job marketplace. Post tasks with a locked reward,
    /// take open tasks, complete them, accept results.
    Job {
        #[command(subcommand)]
        cmd: JobCmd,
    },
}

/// Public tkr devnet defaults — used when --rpc-url / --board aren't
/// passed and the corresponding env var isn't set. These point at the
/// live devnet at tkr.prysm.sh; override for any other chain.
pub const DEFAULT_RPC_URL: &str = "https://tkr.prysm.sh/api/v1/chain/rpc";
pub const DEFAULT_JOB_BOARD: &str = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512";

#[derive(Subcommand, Debug, Clone)]
pub enum JobCmd {
    /// Post a new job. Reward is locked in escrow until the job is
    /// accepted, cancelled, or claimed via timeout.
    Post {
        /// Short ≤256-char preview shown on chain. Full spec is delivered
        /// off-chain (mesh DM) — its keccak goes in `--spec-hash`.
        #[arg(long)]
        preview: String,
        /// keccak256 hash of the full spec, hex 0x...
        #[arg(long)]
        spec_hash: String,
        /// Reward in wei (ETH) or token base units (ERC-20).
        #[arg(long)]
        reward: String,
        /// Token contract address. address(0) = native ETH.
        #[arg(long, default_value = "0x0000000000000000000000000000000000000000")]
        token: String,
        /// Deadline as a unix timestamp in seconds.
        #[arg(long)]
        deadline: u64,
        /// JobBoard contract address. Env: TKR_JOB_BOARD. Default: tkr devnet.
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        /// EVM JSON-RPC URL. Env: TKR_RPC_URL. Default: tkr devnet.
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        /// Path to private key file.
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
    /// List open jobs from the JobBoard.
    List {
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        /// Cap the number of jobs printed.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Take an open job by id.
    Take {
        #[arg(long)]
        id: u64,
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
    /// Submit a completed job's result hash. Worker only.
    Complete {
        #[arg(long)]
        id: u64,
        /// keccak256 hash of the result payload, hex 0x...
        #[arg(long)]
        result_hash: String,
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
    /// Accept a completed job and release the reward to the worker.
    /// Poster only.
    Accept {
        #[arg(long)]
        id: u64,
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
    /// Cancel an Open job (poster only, before any take). Refunds escrow.
    Cancel {
        #[arg(long)]
        id: u64,
        #[arg(long, default_value = DEFAULT_JOB_BOARD)]
        board: String,
        #[arg(long, default_value = DEFAULT_RPC_URL)]
        rpc_url: String,
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum MeshCmd {
    /// Join a mesh via an invite URL (https://.../join/<token> or bare token).
    /// Generates a fresh secp256k1 identity, calls POST /join, persists the
    /// JoinedMesh record to ~/.tkr/mesh/<slug>.json (chmod 0600).
    Join {
        /// Invite URL or bare base64url token from the mesh owner
        #[arg(value_name = "INVITE_URL")]
        url: String,
        /// Optional human-readable name to register with the broker
        #[arg(long)]
        display_name: Option<String>,
    },
    /// List meshes this machine has joined.
    List,
    /// Connect to a joined mesh and tail incoming messages. Run in one
    /// terminal while another peer sends DMs to your address.
    Tail {
        /// Mesh slug (from `tkr mesh list`)
        slug: String,
        /// Reconnect with exponential backoff (1s → 60s, jittered) when the
        /// broker disconnects, instead of exiting. Use this for long-running
        /// daemons where you want the WSS link kept up across broker restarts.
        #[arg(long)]
        reconnect: bool,
    },
    /// Send a plaintext direct message to a peer in the mesh.
    Send {
        /// Mesh slug (from `tkr mesh list`)
        slug: String,
        /// Recipient mesh address (0x... EIP-55)
        #[arg(long)]
        to: String,
        /// Recipient's secp256k1 public key (compressed, 33-byte hex 0x02.../0x03...)
        /// — the recipient prints this with `tkr mesh whoami`.
        #[arg(long)]
        recipient_pubkey: String,
        /// Message body (UTF-8). Use `-` to read from stdin.
        message: String,
    },
    /// Print this peer's mesh address + compressed public key (share the
    /// public key with peers who want to send you DMs).
    Whoami {
        /// Mesh slug (from `tkr mesh list`)
        slug: String,
    },
    /// Mint a signed invite URL. The owner key signs it; share the URL
    /// with anyone you want to admit to the mesh.
    InviteMint {
        /// Short human-readable mesh slug (e.g. "team-alpha")
        #[arg(long)]
        slug: String,
        /// Broker WebSocket URL the invitee will connect to
        /// (e.g. wss://tkr.prysm.sh/api/v1/mesh/ws)
        #[arg(long)]
        broker_url: String,
        /// Path to the mesh owner's private key file (TKR_PAYMENT_KEY=0x...)
        #[arg(long)]
        owner_key_file: std::path::PathBuf,
        /// Invite lifetime in hours. Default: 24.
        #[arg(long, default_value_t = 24)]
        ttl_hours: u64,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PayCmd {
    /// Sign a payment receipt authorizing `cumulative` units for a
    /// session. Output is a JSON object the recipient submits to
    /// MeshEscrow.claim().
    ReceiptIssue {
        /// 32-byte session id, hex 0x... (must match the on-chain channel)
        #[arg(long)]
        session_id: String,
        /// Cumulative paid amount, in token base units (wei for ETH,
        /// 6-dp microUSDC for USDC). Decimal string.
        #[arg(long)]
        cumulative: String,
        /// EVM chain id (8453 = Base mainnet, 84532 = Base sepolia, 31337 = anvil)
        #[arg(long)]
        chain_id: u64,
        /// Address of the deployed MeshEscrow contract
        #[arg(long)]
        contract: String,
        /// Path to a 32-byte hex private key file (no 0x prefix needed)
        #[arg(long)]
        key_file: std::path::PathBuf,
    },
    /// Verify a receipt's signature recovers to the expected payer address.
    ReceiptVerify {
        /// Path to the JSON receipt file (or `-` for stdin)
        #[arg(long)]
        receipt: String,
        /// Expected payer address, EIP-55 0x...
        #[arg(long)]
        payer: String,
    },
    /// Submit a receipt to MeshEscrow.claim() on-chain. Recipient signs the
    /// transaction; their address must match the channel's registered
    /// recipient or the call reverts.
    Claim {
        /// Path to the JSON receipt file (the same shape produced by
        /// `tkr pay receipt-issue`)
        #[arg(long)]
        receipt: String,
        /// EVM JSON-RPC URL (e.g. https://mainnet.base.org or http://127.0.0.1:8545)
        #[arg(long)]
        rpc_url: String,
        /// Path to a 32-byte hex private key file for the recipient
        #[arg(long)]
        key_file: std::path::PathBuf,
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
    /// Claude Code PostToolUse hook. Reads the full hook payload
    /// (`{tool_name, tool_input, tool_response, ...}`) on stdin; records
    /// per-tool size analytics, and emits steering notes via
    /// `hookSpecificOutput.additionalContext` when a tool result was
    /// likely-too-large (e.g., a 5000-line Read). Cannot rewrite the
    /// tool_response itself — that requires an MCP wrapper.
    Post,
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Execute one agent run
    Run {
        /// Path to a TOML manifest
        manifest: std::path::PathBuf,
    },
}
