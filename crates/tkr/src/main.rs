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
    let db = home.join(".tkr/analytics.db");
    if !db.exists() {
        println!("No analytics database to clean.");
        return Ok(());
    }
    if !yes && std::io::stdin().is_terminal() {
        eprint!("Delete {}? [y/N] ", db.display());
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }
    std::fs::remove_file(&db)?;
    println!("Removed {}.", db.display());
    Ok(())
}

fn run_vault_subcommand(sub: &str, extra: &[String]) -> anyhow::Result<()> {
    use host::cli_cmds::vault as vcmd;
    use anyhow::Context;

    let home = dirs::home_dir().unwrap_or_default();
    let vault_root = home.join(".tkr").join("vault");
    let vault = &host::boot::get_host().vault;

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

    let vault = &host::boot::get_host().vault;

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
    let raw_args: Vec<String> = std::env::args().collect();

    // Boot the plugin host eagerly (needed for vault/proxy paths).
    if let Err(e) = host::boot::init() {
        eprintln!("tkr: host boot failed: {e}");
        std::process::exit(1);
    }

    // Route `tkr vault <sub>` and `tkr admin <sub>` directly.
    if raw_args.len() >= 2 {
        match raw_args[1].as_str() {
            "vault" => {
                let sub = raw_args.get(2).map(|s| s.as_str()).unwrap_or("status");
                let extra = raw_args[3..].to_vec();
                return run_vault_subcommand(sub, &extra);
            }
            "admin" => {
                let sub = raw_args.get(2).map(|s| s.as_str()).unwrap_or("help");
                let extra = raw_args[3..].to_vec();
                return run_admin_subcommand(sub, &extra);
            }
            _ => {}
        }
    }

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Watch) => cmds::watch::run(),
        Some(Commands::Gain { breakdown, sort, plain }) => {
            cmds::gain::run(breakdown, &sort, plain)
        }
        Some(Commands::Discover) => cmds::discover::run(),
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
        None => {
            if cli.passthrough.is_empty() {
                eprintln!("Usage: tkr <command> [args...] or tkr --help");
                std::process::exit(1);
            }
            // Wire --max-tokens through env so stream.rs picks it up.
            if let Some(n) = cli.max_tokens {
                std::env::set_var("TKR_MAX_TOKENS", n.to_string());
            }
            let cfg = config::load()?;
            proxy::run(cfg, &cli.passthrough)
        }
    }
}
