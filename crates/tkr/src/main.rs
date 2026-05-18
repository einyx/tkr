#![allow(dead_code)]

mod agent_cmd;
mod cli;
mod cmds;
mod config;
mod dispatch;
mod embedding_ranker;
mod host;
mod native;
mod noise_ranker;
mod proxy;
mod runner;
mod session;
mod signature;
mod stream;
mod tee;
mod util;

use clap::Parser;
use cli::{
    AdminCmd, AgentCmd, Cli, Commands, HookTarget, JobCmd, MeshCmd, PayCmd, SandboxCmd, VaultCmd,
};
use std::io::IsTerminal;

fn vault_main(cmd: Option<VaultCmd>) -> ! {
    if let Err(e) = host::boot::ensure_full() {
        eprintln!("tkr: host boot failed: {e}");
        std::process::exit(1);
    }
    match vault_run(cmd.unwrap_or(VaultCmd::Status)) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("tkr vault: {e:#}");
            std::process::exit(1);
        }
    }
}

fn vault_run(cmd: VaultCmd) -> anyhow::Result<i32> {
    use anyhow::Context;
    use host::cli_cmds::vault as vcmd;

    let home = dirs::home_dir().unwrap_or_default();
    let vault_root = home.join(".tkr").join("vault");
    let vault_arc = host::boot::vault();
    let vault = &vault_arc;

    match cmd {
        VaultCmd::Status => vcmd::status(vault),
        VaultCmd::Unseal => vcmd::unseal(vault),
        VaultCmd::Seal => vcmd::seal(vault),
        VaultCmd::Init => {
            std::fs::create_dir_all(&vault_root).context("create vault dir")?;
            vcmd::init(&vault_root, vcmd::InitMode::MasterKeyFile)
        }
        VaultCmd::Rotate => {
            let new_master = vcmd::rotate(vault, &vault_root)?;
            let vault_root_str = vault_root.to_string_lossy().into_owned();
            if let Err(e) =
                host::vault::keychain::set_master_key("tkr-vault", &vault_root_str, &new_master)
            {
                // Never print the raw key — stderr is captured by terminal
                // scrollback, tmux logs, and parent processes (e.g. CI,
                // Claude Code). Direct the operator to back up the
                // already-persisted master.key file by other means.
                let _ = new_master;
                eprintln!("tkr: error: could not persist master key after rotate: {e}");
                eprintln!(
                    "tkr: the new key was generated and the vault re-encrypted, but \
                     the OS keyring write failed."
                );
                eprintln!(
                    "tkr: copy {} to a safe location now (file is mode 0600).",
                    vault_root.join("master.key").display()
                );
                return Ok(1);
            }
            Ok(0)
        }
        VaultCmd::Export { path } => {
            let p = path.unwrap_or_else(|| vault_root.with_extension("tar.gz"));
            vcmd::export(&vault_root, &p)
        }
        VaultCmd::Import { bundle } => vcmd::import(bundle.as_path(), &vault_root),
        VaultCmd::Audit { verify, last } => vcmd::audit(
            &vault_root,
            vcmd::AuditOpts {
                verify,
                last_n: last,
            },
        ),
    }
}

fn admin_main(cmd: AdminCmd) -> ! {
    if let Err(e) = host::boot::ensure_full() {
        eprintln!("tkr: host boot failed: {e}");
        std::process::exit(1);
    }
    use host::cli_cmds::admin;
    let vault_arc = host::boot::vault();
    let vault = &vault_arc;
    let AdminCmd::Reset { plugin } = cmd;
    match admin::reset(vault, &plugin) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("tkr admin: {e:#}");
            std::process::exit(1);
        }
    }
}

fn clean_stats(yes: bool) -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_default();
    let legacy_db = home.join(".tkr/analytics.db");
    let migrated_db = home.join(".tkr/analytics.db.migrated");
    let vault_dir = home.join(".tkr/vault");

    let mut targets: Vec<std::path::PathBuf> = Vec::new();
    for p in [&legacy_db, &migrated_db, &vault_dir] {
        if p.exists() {
            targets.push(p.clone());
        }
    }

    if targets.is_empty() {
        println!("Nothing to clean.");
        return Ok(());
    }

    if !yes && std::io::stdin().is_terminal() {
        eprintln!("Will remove:");
        for p in &targets {
            eprintln!("  {}", p.display());
        }
        eprint!("Continue? [y/N] ");
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    for p in &targets {
        let r = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        match r {
            Ok(_) => println!("Removed {}.", p.display()),
            Err(e) => eprintln!("Failed to remove {}: {e}", p.display()),
        }
    }
    Ok(())
}

/// If clap did not match a subcommand but the user typed `tkr update …`, run the
/// self-updater instead of proxying to a non-existent binary named `update`
/// (builds without the `Update` variant, or rare parser edge cases).
fn dispatch_update_from_passthrough(cli: &Cli) -> Option<anyhow::Result<()>> {
    if cli.command.is_some() {
        return None;
    }
    if cli.passthrough.first().map(|s| s.as_str()) != Some("update") {
        return None;
    }
    let mut check = false;
    let mut force = false;
    for arg in cli.passthrough.iter().skip(1) {
        match arg.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            _ => {
                return Some(Err(anyhow::anyhow!(
                    "unknown argument to `tkr update`: {arg}"
                )));
            }
        }
    }
    Some(cmds::update::run(check, force))
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(res) = dispatch_update_from_passthrough(&cli) {
        return res;
    }

    // Commands that touch the vault/plugins boot the host first.
    // gain/suggest/watch/discover need the full vault boot.
    // The proxy path (None) uses the fast filter-only boot (called inside proxy::run).
    // Pure paths (Version, Rewrite, Update, Install, Hook, CleanStats, Bench, Agent)
    // skip boot entirely and stay sub-50ms.
    let needs_full_boot = matches!(
        cli.command,
        Some(Commands::Watch)
            | Some(Commands::Gain { .. })
            | Some(Commands::Discover { .. })
            | Some(Commands::Suggest)
            | Some(Commands::Explain { .. })
    );
    if needs_full_boot {
        if let Err(e) = host::boot::ensure_full() {
            eprintln!("tkr: host boot failed: {e}");
            std::process::exit(1);
        }
    }

    match cli.command {
        Some(Commands::Vault { cmd }) => vault_main(cmd),
        Some(Commands::Admin { cmd }) => admin_main(cmd),
        Some(Commands::Pay { cmd }) => match cmd {
            PayCmd::ReceiptIssue {
                session_id,
                cumulative,
                chain_id,
                contract,
                key_file,
            } => cmds::pay::receipt_issue(
                &session_id,
                &cumulative,
                chain_id,
                &contract,
                &key_file,
            ),
            PayCmd::ReceiptVerify { receipt, payer } => {
                cmds::pay::receipt_verify(&receipt, &payer)
            }
            PayCmd::Claim { receipt, rpc_url, key_file } => {
                cmds::pay::claim(&receipt, &rpc_url, &key_file)
            }
        },
        Some(Commands::Mesh { cmd }) => match cmd {
            MeshCmd::Join { url, display_name } => {
                cmds::mesh::join(&url, display_name.as_deref())
            }
            MeshCmd::List => cmds::mesh::list(),
            MeshCmd::Whoami { slug } => cmds::mesh::whoami(&slug),
            MeshCmd::Tail { slug, reconnect } => cmds::mesh::tail(&slug, reconnect),
            MeshCmd::Send { slug, to, recipient_pubkey, message } => {
                cmds::mesh::send(&slug, &to, &recipient_pubkey, &message)
            }
            MeshCmd::InviteMint { slug, broker_url, owner_key_file, ttl_hours } => {
                cmds::mesh::invite_mint(&slug, &broker_url, &owner_key_file, ttl_hours)
            }
        },
        Some(Commands::Mcp) => tkr_mcp::Server::run(),
        Some(Commands::Sandbox { cmd }) => match cmd {
            SandboxCmd::Run {
                read,
                write,
                env,
                memory,
                cpu,
                timeout_ms,
                max_output,
                no_network,
                allow_connect,
                allow_bind,
                argv,
            } => cmds::sandbox::run(
                read,
                write,
                env,
                memory,
                cpu,
                timeout_ms,
                max_output,
                no_network,
                allow_connect,
                allow_bind,
                argv,
            ),
        },
        Some(Commands::Job { cmd }) => match cmd {
            JobCmd::Post { preview, spec_hash, reward, token, deadline, board, rpc_url, key_file } => {
                cmds::jobs::post(&preview, &spec_hash, &reward, &token, deadline, &board, &rpc_url, &key_file)
            }
            JobCmd::List { board, rpc_url, limit } => cmds::jobs::list(&board, &rpc_url, limit),
            JobCmd::Take { id, board, rpc_url, key_file } => cmds::jobs::take(id, &board, &rpc_url, &key_file),
            JobCmd::Complete { id, result_hash, board, rpc_url, key_file } => {
                cmds::jobs::complete(id, &result_hash, &board, &rpc_url, &key_file)
            }
            JobCmd::Accept { id, board, rpc_url, key_file } => cmds::jobs::accept(id, &board, &rpc_url, &key_file),
            JobCmd::Cancel { id, board, rpc_url, key_file } => cmds::jobs::cancel(id, &board, &rpc_url, &key_file),
        },
        Some(Commands::Watch) => cmds::watch::run(),
        Some(Commands::Gain {
            breakdown,
            sort,
            plain,
        }) => cmds::gain::run(breakdown, &sort, plain),
        Some(Commands::Discover { history, limit }) => cmds::discover::run(history, limit),
        Some(Commands::Suggest) => cmds::suggest::run(),
        Some(Commands::Explain { file }) => cmds::explain::run(file),
        Some(Commands::Rewrite { command }) => cmds::rewrite::run(&command),
        Some(Commands::Hook { target }) => match target {
            HookTarget::Claude => cmds::hook::run_claude(),
            HookTarget::Universal => cmds::hook::run_universal(),
            HookTarget::Post => cmds::hook::run_post(),
        },
        Some(Commands::Version) => {
            println!("tkr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Commands::CleanStats { yes }) => clean_stats(yes),
        Some(Commands::Install { claude, codex, cursor, with_foundry }) => cmds::install::run(claude, codex, cursor, with_foundry),
        Some(Commands::Uninstall { claude, codex, cursor }) => cmds::install::uninstall(claude, codex, cursor),
        Some(Commands::Bench { command }) => cmds::bench::run(&command),
        Some(Commands::Agent { cmd }) => match cmd {
            AgentCmd::Run { manifest } => agent_cmd::run_agent(&manifest),
        },
        Some(Commands::Update { check, force }) => cmds::update::run(check, force),
        None => {
            if cli.passthrough.is_empty() {
                eprintln!("Usage: tkr <command> [args...] or tkr --help");
                std::process::exit(1);
            }
            // Wire --max-tokens / --compact-json through env so stream.rs sees them.
            if let Some(n) = cli.max_tokens {
                std::env::set_var("TKR_MAX_TOKENS", n.to_string());
            }
            if cli.compact_json {
                std::env::set_var("TKR_COMPACT_JSON", "1");
            }
            let cfg = config::load()?;
            proxy::run(cfg, &cli.passthrough)
        }
    }
}
