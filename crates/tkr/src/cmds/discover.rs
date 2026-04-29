use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

static FALLBACK_COMMANDS: &[&str] = &[
    "git", "cargo", "npm", "pnpm", "yarn", "jest", "vitest",
    "docker", "kubectl", "terraform", "make",
];

pub fn run(history_override: Option<PathBuf>, limit: usize) -> Result<()> {
    let home = dirs::home_dir().unwrap_or_default();

    println!("tkr discover — scanning for savings opportunities\n");

    let history_path = history_override
        .or_else(default_history_path)
        .unwrap_or_else(|| home.join(".zsh_history"));
    let history = std::fs::read_to_string(&history_path).unwrap_or_default();
    let commands = discoverable_commands();
    let start_at = history.lines().count().saturating_sub(limit);

    let mut missed: HashMap<String, u64> = HashMap::new();
    for line in history.lines().skip(start_at) {
        let line = line.trim_start_matches(|c: char| c == ':' || c.is_ascii_digit() || c == ';' || c == ' ');
        for cmd in &commands {
            if line.starts_with(cmd) && !line.starts_with("tkr ") {
                *missed.entry(cmd.clone()).or_default() += 1;
            }
        }
    }

    if missed.is_empty() {
        println!("Great — no obvious missed opportunities found in shell history.");
    } else {
        let mut sorted: Vec<_> = missed.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        println!("Commands run without tkr (estimated missed savings):\n");
        for (cmd, count) in sorted {
            println!("  {cmd:<20} {count:>6} times — try: tkr {cmd} ...");
        }
        println!(
            "\nScanned {} commands from filters/fallbacks over last {} history lines ({})",
            commands.len(),
            limit,
            history_path.display()
        );
        println!("\nTip: add `alias git='tkr git'` (and similar) to your shell profile to proxy automatically.");
    }

    Ok(())
}

fn default_history_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let zsh = home.join(".zsh_history");
    let bash = home.join(".bash_history");
    if zsh.exists() {
        Some(zsh)
    } else if bash.exists() {
        Some(bash)
    } else {
        None
    }
}

fn discoverable_commands() -> Vec<String> {
    let mut out: HashSet<String> = HashSet::new();
    for c in FALLBACK_COMMANDS {
        out.insert((*c).to_string());
    }
    for p in candidate_filter_dirs() {
        if !p.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&p) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "toml") {
                    if let Some(cmd) = parse_filter_command(&path) {
                        out.insert(cmd);
                    }
                }
            }
        }
    }
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v
}

fn candidate_filter_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = crate::config::bundled_filters_dir() {
        dirs.push(p);
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".tkr/filters"));
    }
    dirs
}

fn parse_filter_command(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("command")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
}
