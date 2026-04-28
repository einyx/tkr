mod cli;
mod cmds;
mod config;
mod dispatch;
mod proxy;
mod runner;
mod session;
mod stream;

use clap::Parser;
use cli::{Cli, Commands};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Watch) => cmds::watch::run(),
        Some(Commands::Gain { breakdown }) => cmds::gain::run(breakdown),
        Some(Commands::Discover) => cmds::discover::run(),
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
