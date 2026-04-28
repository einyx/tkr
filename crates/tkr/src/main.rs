mod cli;
mod cmds;
mod config;
mod dispatch;
mod proxy;
mod runner;
mod session;
mod stream;

use clap::Parser;
use cli::{Cli, Commands, HookTarget};
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

fn main() -> anyhow::Result<()> {
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
