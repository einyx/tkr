use anyhow::Result;

static KNOWN_COMMANDS: &[&str] = &[
    "git", "cargo", "npm", "pnpm", "yarn", "jest", "vitest",
    "docker", "kubectl", "terraform", "make",
];

pub fn run() -> Result<()> {
    let home = dirs::home_dir().unwrap_or_default();

    println!("tkr discover — scanning for savings opportunities\n");

    let history_file = home.join(".zsh_history");
    let alt_history = home.join(".bash_history");
    let history = if history_file.exists() {
        std::fs::read_to_string(&history_file).unwrap_or_default()
    } else {
        std::fs::read_to_string(&alt_history).unwrap_or_default()
    };

    let mut missed: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for line in history.lines() {
        let line = line.trim_start_matches(|c: char| c == ':' || c.is_ascii_digit() || c == ';' || c == ' ');
        for cmd in KNOWN_COMMANDS {
            if line.starts_with(cmd) && !line.starts_with("tkr ") {
                *missed.entry(cmd).or_default() += 1;
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
        println!("\nTip: add `alias git='tkr git'` (and similar) to your shell profile to proxy automatically.");
    }

    Ok(())
}
