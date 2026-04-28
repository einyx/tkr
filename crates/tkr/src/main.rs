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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Watch) => cmds::watch::run(),
        Some(Commands::Gain { breakdown, sort, plain }) => {
            cmds::gain::run(breakdown, &sort, plain)
        }
        Some(Commands::Discover) => cmds::discover::run(),
        Some(Commands::Rewrite { command }) => cmds::rewrite::run(&command),
        Some(Commands::Hook { target }) => match target {
            HookTarget::Claude => cmds::hook::run_claude(),
        },
        Some(Commands::Version) => {
            println!("tkr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            if cli.passthrough.is_empty() {
                eprintln!("Usage: tkr <command> [args...] or tkr --help");
                std::process::exit(1);
            }
            let cfg = config::load()?;
            proxy::run(cfg, &cli.passthrough)
        }
    }
}
