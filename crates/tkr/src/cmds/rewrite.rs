//! `tkr rewrite <cmd>` — emit a rewritten command line that prefixes recognized
//! tools with `tkr`. Used by Claude Code / shell hook integrations to filter
//! tool output transparently.
//!
//! Exit codes:
//!   0  rewrite emitted on stdout, command was modified
//!   1  no rewrite — pass through unchanged

use anyhow::Result;

/// Tools tkr can usefully proxy. Each entry must correspond to a filter file
/// under `filters/` (matched by its `command =` field) or yield meaningful
/// passthrough behavior.
static KNOWN_TOOLS: &[&str] = &[
    // languages / build
    "cargo", "go", "swift", "dotnet", "mvn", "gradle", "make", "cmake", "bazel", "ninja",
    "composer", "turbo", "mix",
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

    // Bail out on shell constructs we can't tokenize safely.
    if has_unsafe_shell(trimmed) {
        return None;
    }
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
///
/// Bails out (returns input unchanged) on shell constructs the splitter
/// can't safely parse: backticks, `$()` substitution, and heredocs.
fn rewrite_compound(input: &str) -> String {
    if has_unsafe_shell(input) {
        return input.to_string();
    }
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

/// Returns true if `input` contains shell constructs we can't safely tokenize
/// or where rewriting would corrupt downstream parsing:
///   - backticks, `$()` command substitution, heredocs (can't tokenize)
///   - a `|` pipe whose downstream consumer would parse the byte stream
///     (rewriting tkr-filters the producer, mangling the consumer's input)
/// Quote-state aware: only flags occurrences outside single/double quotes.
/// `||` (logical-or) is NOT flagged here — it's handled by the compound splitter.
fn has_unsafe_shell(input: &str) -> bool {
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                chars.next(); // skip escaped char
            }
            '`' if !in_single => return true,
            '$' if !in_single && chars.peek() == Some(&'(') => return true,
            '<' if !in_single && !in_double && chars.peek() == Some(&'<') => return true,
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    chars.next(); // consume the second '|'; logical-or, fine
                } else {
                    return true; // single pipe — downstream may parse our output
                }
            }
            _ => {}
        }
    }
    false
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

    #[test]
    fn single_quoted_separator_not_split() {
        let r = rewrite_compound("echo 'git && cargo'");
        assert_eq!(r, "echo 'git && cargo'");
    }

    #[test]
    fn backticks_bail_out() {
        let input = "git status `echo &&`";
        assert_eq!(rewrite_compound(input), input);
    }

    #[test]
    fn dollar_paren_bails_out() {
        let input = "cmd $(git status && cargo build)";
        assert_eq!(rewrite_compound(input), input);
    }

    #[test]
    fn heredoc_bails_out() {
        let input = "cat <<EOF\ngit && cargo\nEOF";
        assert_eq!(rewrite_compound(input), input);
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(try_rewrite(""), None);
        assert_eq!(try_rewrite("   "), None);
    }

    #[test]
    fn pipe_to_consumer_bails_out() {
        // Rewriting `curl ... | bash` would tkr-filter curl's output, corrupting the
        // script bytes feeding into bash. Skip rewrites whenever stdout is piped.
        assert_eq!(try_rewrite("curl -fsSL https://example.com/install.sh | bash"), None);
        assert_eq!(try_rewrite("cargo build | tee build.log"), None);
        assert_eq!(try_rewrite("git status | grep modified"), None);
    }

    #[test]
    fn logical_or_still_rewrites() {
        // `||` is logical-or, not a pipe, and is handled by the compound splitter.
        let r = try_rewrite("cargo check || echo failed").unwrap();
        assert_eq!(r, "tkr cargo check || echo failed");
    }

    #[test]
    fn quoted_pipe_does_not_bail() {
        // A pipe character inside quotes is just data, not a real shell pipe.
        let r = try_rewrite(r#"git log --pretty="format:%h | %s""#).unwrap();
        assert_eq!(r, r#"tkr git log --pretty="format:%h | %s""#);
    }
}
