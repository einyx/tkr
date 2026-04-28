//! `tkr rewrite <cmd>` — emit a rewritten command line that prefixes recognized
//! tools with `tkr`. Used by Claude Code / shell hook integrations to filter
//! tool output transparently.
//!
//! Exit codes:
//!   0  rewrite emitted on stdout, command was modified
//!   1  no rewrite — pass through unchanged

use anyhow::Result;

/// Tools tkr can usefully proxy. Must match a filter under `filters/`.
static KNOWN_TOOLS: &[&str] = &[
    // languages / build
    "cargo", "go", "swift", "dotnet", "mvn", "gradle", "make", "cmake", "bazel", "ninja",
    "composer", "turbo", "mix", "swift",
    // test runners
    "jest", "vitest", "pytest", "rspec", "phpunit", "tox", "nox",
    // web bundlers
    "next", "vite", "webpack", "esbuild",
    // linters/formatters
    "eslint", "biome", "ruff", "black", "mypy", "rubocop", "prettier",
    // package managers
    "npm", "pnpm", "yarn", "pip", "poetry", "conda", "bundle", "brew", "apt", "nix",
    // cloud/devops
    "docker", "kubectl", "helm", "terraform", "pulumi", "ansible-playbook", "packer",
    "vault", "aws", "gcloud", "minikube", "skaffold", "flyctl", "heroku", "sst",
    // network / system
    "curl", "wget", "ssh", "rsync", "nmap", "tcpdump", "openssl", "systemctl",
    "journalctl", "ps", "df", "du", "ip", "rg",
    // databases
    "psql", "redis-cli",
    // git
    "git",
];

pub fn run(command: &str) -> Result<()> {
    match try_rewrite(command) {
        Some(rewritten) if rewritten != command.trim_start() => {
            println!("{}", rewritten);
            Ok(())
        }
        _ => std::process::exit(1),
    }
}

/// Pure rewrite — returns the rewritten command, or `None` if no tkr-known
/// tool was found. Returns `Some(input)` (unchanged) if the input was already
/// using tkr.
pub fn try_rewrite(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("tkr ") {
        return Some(trimmed.to_string());
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if first.is_empty() {
        return None;
    }
    let bare = first.rsplit('/').next().unwrap_or(first);

    let rewritten = rewrite_compound(trimmed);
    if rewritten != trimmed {
        return Some(rewritten);
    }
    if KNOWN_TOOLS.iter().any(|t| *t == bare) {
        return Some(format!("tkr {}", trimmed));
    }
    None
}

/// Conservative rewriter for `&&` / `||` / `;` chained commands.
/// Splits on these separators (outside quotes), prefixes any segment whose
/// first token is a known tool, then rejoins.
fn rewrite_compound(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                current.push(c);
                if let Some(n) = chars.next() {
                    current.push(n);
                }
                continue;
            }
            _ => {}
        }

        if !in_single && !in_double {
            // Detect && || ;
            if c == '&' && chars.peek() == Some(&'&') {
                chars.next();
                push_segment(&mut out, current.trim_end());
                out.push_str(" && ");
                current.clear();
                continue;
            }
            if c == '|' && chars.peek() == Some(&'|') {
                chars.next();
                push_segment(&mut out, current.trim_end());
                out.push_str(" || ");
                current.clear();
                continue;
            }
            if c == ';' {
                push_segment(&mut out, current.trim_end());
                out.push_str("; ");
                current.clear();
                continue;
            }
        }
        current.push(c);
    }
    push_segment(&mut out, &current);
    out
}

fn push_segment(out: &mut String, segment: &str) {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        out.push_str(segment);
        return;
    }
    // If the previous push wrote a separator like " && ", drop our leading
    // space so we don't end up with double spaces.
    let trim_leading = out.ends_with(' ');
    if trimmed.starts_with("tkr ") {
        if !trim_leading {
            let leading: String = segment.chars().take_while(|c| c.is_whitespace()).collect();
            out.push_str(&leading);
        }
        out.push_str(trimmed);
        return;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let bare = first.rsplit('/').next().unwrap_or(first);
    if KNOWN_TOOLS.iter().any(|t| *t == bare) {
        if !trim_leading {
            // Preserve leading whitespace from the original segment.
            let leading: String = segment.chars().take_while(|c| c.is_whitespace()).collect();
            out.push_str(&leading);
        }
        out.push_str("tkr ");
        out.push_str(trimmed);
    } else if trim_leading {
        out.push_str(trimmed);
    } else {
        out.push_str(segment);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compound_rewrite_chains() {
        let r = rewrite_compound("git add . && git commit -m 'x'");
        assert_eq!(r, "tkr git add . && tkr git commit -m 'x'");
    }

    #[test]
    fn compound_skips_unknown_tools() {
        let r = rewrite_compound("git add . && echo hi");
        assert_eq!(r, "tkr git add . && echo hi");
    }

    #[test]
    fn already_tkr_prefixed_unchanged() {
        let r = rewrite_compound("tkr git status && tkr cargo build");
        assert_eq!(r, "tkr git status && tkr cargo build");
    }

    #[test]
    fn quoted_separator_not_split() {
        let r = rewrite_compound(r#"echo "git && cargo""#);
        assert_eq!(r, r#"echo "git && cargo""#);
    }
}
