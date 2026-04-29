#![allow(dead_code)]

mod agent_cmd;
mod cli;
mod cmds;
mod config;
mod dispatch;
mod embedding_ranker;
mod host;
mod noise_ranker;
mod proxy;
mod runner;
mod session;
mod signature;
mod stream;
mod util;

use clap::Parser;
use cli::{AdminCmd, AgentCmd, Cli, Commands, HookTarget, VaultCmd};
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
                eprintln!("tkr: warning: could not persist master key after rotate: {e}");
                eprintln!("tkr: new master key (hex) — store manually:");
                eprintln!("  {}", hex::encode(new_master));
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

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
        },
        Some(Commands::Version) => {
            println!("tkr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Commands::CleanStats { yes }) => clean_stats(yes),
        Some(Commands::Install { claude, codex }) => cmds::install::run(claude, codex),
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
