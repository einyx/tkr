//! Best-effort secret scrubber for the recorded `output_preview`. The vault
//! is encrypted at rest, but anyone with vault-read access (or possession of
//! `~/.tkr/vault/.tkr-vault.key`) sees the cleartext — so commands that
//! routinely emit credentials must never be previewed verbatim.
//!
//! Two layers:
//!
//! 1. **Command denylist** — `cat`, `env`, `printenv`, `echo`, `printf`,
//!    `dotenv`, `aws sts get-session-token`, etc. emit so much credential
//!    material so often that we don't even attempt a preview; we record
//!    `<preview suppressed: command in deny-list>`.
//!
//! 2. **Line-level pattern scrubber** — for everything else, lines matching
//!    common secret patterns (`sk-…`, `Bearer …`, `BEGIN PRIVATE`,
//!    `AKIA…`, common `KEY=…` shapes) are replaced with `<redacted>`.
//!
//! These are heuristics, not guarantees. The goal is to keep the *most
//! common* secret-leak shapes out of long-lived recordings — not to defeat
//! a determined attacker who can choose what to print.

/// Names of commands whose stdout we never preview. Conservative: better
/// to lose a preview than to record an `AWS_SECRET_ACCESS_KEY=...` line.
const DENY_COMMANDS: &[&str] = &[
    "cat", "env", "printenv", "echo", "printf", "dotenv", "direnv",
    "set", "export", "ssh-add", "gpg", "openssl", "keychain", "security",
    "pass", "bw", "op", "lpass", "vault", "kubeseal",
];

/// True if the recorder should suppress `output_preview` entirely for a
/// command with this binary name.
pub fn is_deny_listed(command: &str) -> bool {
    let base = command.rsplit('/').next().unwrap_or(command);
    let base = base.split_whitespace().next().unwrap_or(base);
    DENY_COMMANDS.iter().any(|d| *d == base)
}

/// Scrub a single line. Returns the original line unchanged if no pattern
/// matches; otherwise `<redacted: reason>`.
pub fn scrub_line(line: &str) -> String {
    if let Some(reason) = match_secret_pattern(line) {
        format!("<redacted: {reason}>")
    } else {
        line.to_string()
    }
}

fn match_secret_pattern(line: &str) -> Option<&'static str> {
    // PEM blocks
    if line.contains("-----BEGIN ") && line.contains("PRIVATE KEY") {
        return Some("pem private key");
    }
    if line.contains("-----BEGIN OPENSSH PRIVATE KEY") {
        return Some("openssh private key");
    }

    // AWS access key shape (AKIA / ASIA + 16 uppercase alphanums).
    if let Some(idx) = line.find("AKIA").or_else(|| line.find("ASIA")) {
        let tail = &line[idx + 4..];
        let count = tail
            .chars()
            .take(16)
            .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .count();
        if count == 16 {
            return Some("aws access key id");
        }
    }

    // Anthropic / OpenAI / common provider key prefixes.
    for prefix in [
        "sk-ant-",
        "sk-proj-",
        "sk-",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "gho_",
        "ghs_",
        "github_pat_",
        "glpat-",
        "AIza",
    ] {
        if let Some(after) = line.find(prefix) {
            let tail = &line[after + prefix.len()..];
            let token_len = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .count();
            if token_len >= 16 {
                return Some("api key");
            }
        }
    }

    // `Bearer <token>` (HTTP auth header).
    if let Some(after) = line
        .find("Bearer ")
        .or_else(|| line.find("bearer "))
        .or_else(|| line.find("BEARER "))
    {
        let tail = &line[after + 7..];
        let token_len = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .count();
        if token_len >= 16 {
            return Some("bearer token");
        }
    }

    // KEY=value style env-var dumps. Match conservative key-name suffixes
    // so we don't redact every `=` in source code.
    let lower = line.to_ascii_lowercase();
    let env_markers = [
        "_token=",
        "_secret=",
        "_password=",
        "_passwd=",
        "_api_key=",
        "_apikey=",
        "_access_key=",
        "_private_key=",
        "_session_token=",
        "anthropic_api_key=",
        "openai_api_key=",
        "aws_secret_access_key=",
        "github_token=",
        "slack_token=",
    ];
    for marker in env_markers {
        if let Some(idx) = lower.find(marker) {
            let tail = &line[idx + marker.len()..];
            let val = tail
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '\'')
                .count();
            if val >= 8 {
                return Some("credential env var");
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_lists_common_credential_emitters() {
        assert!(is_deny_listed("cat"));
        assert!(is_deny_listed("env"));
        assert!(is_deny_listed("/usr/bin/printenv"));
        assert!(is_deny_listed("op"));
        assert!(!is_deny_listed("ls"));
        assert!(!is_deny_listed("git"));
    }

    #[test]
    fn scrubs_anthropic_key() {
        let line = "ANTHROPIC_API_KEY=sk-ant-api03-abcdefghijklmnop1234567";
        let out = scrub_line(line);
        assert!(out.starts_with("<redacted"), "got: {out}");
    }

    #[test]
    fn scrubs_openai_key() {
        let line = "export OPENAI_API_KEY=sk-proj-AAAAAAAAAAAAAAAAAAAAAAAA";
        assert!(scrub_line(line).starts_with("<redacted"));
    }

    #[test]
    fn scrubs_github_pat() {
        let line = "GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(scrub_line(line).starts_with("<redacted"));
    }

    #[test]
    fn scrubs_aws_access_key() {
        let line = "  AccessKey: AKIAIOSFODNN7EXAMPLE";
        assert!(scrub_line(line).starts_with("<redacted"));
    }

    #[test]
    fn scrubs_bearer_token() {
        let line = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpX";
        assert!(scrub_line(line).starts_with("<redacted"));
    }

    #[test]
    fn scrubs_pem_block_header() {
        assert!(scrub_line("-----BEGIN RSA PRIVATE KEY-----").starts_with("<redacted"));
        assert!(scrub_line("-----BEGIN OPENSSH PRIVATE KEY-----").starts_with("<redacted"));
    }

    #[test]
    fn passes_normal_command_output_through() {
        let line = "fn main() { println!(\"hello\"); }";
        assert_eq!(scrub_line(line), line);
    }

    #[test]
    fn passes_short_alphanumerics_through() {
        // sk- prefix exists in the wild outside of api keys (e.g. some IDs).
        // Require at least 16 chars after the prefix.
        let line = "let x = sk-3";
        assert_eq!(scrub_line(line), line);
    }
}
