mod agent_cmd;
mod cli;
mod cmds;
mod host;
mod config;
mod dispatch;
mod proxy;
mod runner;
mod session;
mod embedding_ranker;
mod noise_ranker;
mod signature;
mod stream;
mod util;

use clap::Parser;
use cli::{AgentCmd, Cli, Commands, HookTarget};
use std::io::IsTerminal;

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

fn run_vault_subcommand(sub: &str, extra: &[String]) -> anyhow::Result<()> {
    use host::cli_cmds::vault as vcmd;
    use anyhow::Context;

    let home = dirs::home_dir().unwrap_or_default();
    let vault_root = home.join(".tkr").join("vault");
    let vault_arc = host::boot::vault();
    let vault = &vault_arc;

    let exit = match sub {
        "status" => vcmd::status(vault)?,
        "unseal" => vcmd::unseal(vault)?,
        "seal"   => vcmd::seal(vault)?,
        "init"   => {
            std::fs::create_dir_all(&vault_root).context("create vault dir")?;
            vcmd::init(&vault_root, vcmd::InitMode::Keychain)?
        }
        "rotate" => {
            let new_master = vcmd::rotate(vault, &vault_root)?;
            // Persist the new master key to keychain.
            let vault_root_str = vault_root.to_string_lossy().into_owned();
            if let Err(e) = host::vault::keychain::set_master_key("tkr-vault", &vault_root_str, &new_master) {
                eprintln!("tkr: warning: could not update keychain after rotate: {e}");
                eprintln!("tkr: new master key (hex) — store manually:");
                eprintln!("  {}", hex::encode(new_master));
            }
            0
        }
        "export" => {
            let path = extra.first().map(|s| std::path::PathBuf::from(s))
                .unwrap_or_else(|| vault_root.with_extension("tar.gz"));
            vcmd::export(&vault_root, &path)?
        }
        "import" => {
            let bundle = extra.first()
                .ok_or_else(|| anyhow::anyhow!("usage: tkr vault import <bundle.tar.gz>"))?;
            vcmd::import(std::path::Path::new(bundle), &vault_root)?
        }
        "audit" => {
            let verify = extra.iter().any(|a| a == "--verify");
            let last_n = extra.iter().position(|a| a == "--last")
                .and_then(|i| extra.get(i + 1))
                .and_then(|n| n.parse::<usize>().ok());
            vcmd::audit(&vault_root, vcmd::AuditOpts { verify, last_n })?
        }
        other => {
            eprintln!("tkr vault: unknown subcommand '{other}'");
            eprintln!("usage: tkr vault {{status|init|unseal|seal|rotate|export|import|audit}}");
            1
        }
    };
    std::process::exit(exit);
}

fn run_admin_subcommand(sub: &str, extra: &[String]) -> anyhow::Result<()> {
    use host::cli_cmds::admin;

    let vault_arc = host::boot::vault();
    let vault = &vault_arc;

    let exit = match sub {
        "reset" => {
            // Parse: tkr admin reset --plugin <name>
            let plugin = extra.windows(2)
                .find(|w| w[0] == "--plugin")
                .map(|w| w[1].as_str())
                .ok_or_else(|| anyhow::anyhow!("usage: tkr admin reset --plugin <name>"))?;
            admin::reset(vault, plugin)?
        }
        other => {
            eprintln!("tkr admin: unknown subcommand '{other}'");
            eprintln!("usage: tkr admin {{reset --plugin <name>}}");
            1
        }
    };
    std::process::exit(exit);
}

fn main() -> anyhow::Result<()> {
    // Peek at raw args before clap parsing so we can handle `tkr vault ...`
    // and `tkr admin ...` (not registered as clap subcommands in cli.rs).
    //
    // Host boot is lazy: only command paths that touch the vault/plugins
    // (proxy, gain, suggest, watch, vault, admin) call host::boot::ensure().
    // Pure paths like `tkr version`, `tkr rewrite`, `tkr update`, `tkr install`,
    // `tkr --help` skip the ~1s boot cost. The hook calls `tkr rewrite` on
    // every Bash command, so this matters a lot.
    let raw_args: Vec<String> = std::env::args().collect();

    // Route `tkr vault <sub>` and `tkr admin <sub>` directly. These need the full host.
    if raw_args.len() >= 2 {
        match raw_args[1].as_str() {
            "vault" => {
                if let Err(e) = host::boot::ensure_full() {
                    eprintln!("tkr: host boot failed: {e}");
                    std::process::exit(1);
                }
                let sub = raw_args.get(2).map(|s| s.as_str()).unwrap_or("status");
                let extra = if raw_args.len() > 3 { raw_args[3..].to_vec() } else { Vec::new() };
                return run_vault_subcommand(sub, &extra);
            }
            "admin" => {
                if let Err(e) = host::boot::ensure_full() {
                    eprintln!("tkr: host boot failed: {e}");
                    std::process::exit(1);
                }
                let sub = raw_args.get(2).map(|s| s.as_str()).unwrap_or("help");
                let extra = if raw_args.len() > 3 { raw_args[3..].to_vec() } else { Vec::new() };
                return run_admin_subcommand(sub, &extra);
            }
            _ => {}
        }
    }

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
    );
    if needs_full_boot {
        if let Err(e) = host::boot::ensure_full() {
            eprintln!("tkr: host boot failed: {e}");
            std::process::exit(1);
        }
    }

    match cli.command {
        Some(Commands::Watch) => cmds::watch::run(),
        Some(Commands::Gain { breakdown, sort, plain }) => {
            cmds::gain::run(breakdown, &sort, plain)
        }
        Some(Commands::Discover { history, limit }) => cmds::discover::run(history, limit),
        Some(Commands::Suggest) => cmds::suggest::run(),
        Some(Commands::Rewrite { command }) => cmds::rewrite::run(&command),
        Some(Commands::Hook { target }) => match target {
            HookTarget::Claude => cmds::hook::run_claude(),
        },
        Some(Commands::Version) => {
            println!("tkr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Commands::CleanStats { yes }) => clean_stats(yes),
        Some(Commands::Install) => cmds::install::run(),
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
