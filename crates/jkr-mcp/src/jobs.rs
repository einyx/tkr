//! Read-only JobBoard MCP tool. Wraps `tkr jobs list`.
//!
//! We shell out to the `tkr` binary instead of pulling alloy/EVM-RPC into
//! tkr-mcp. The tradeoff: tkr-mcp must find a tkr binary on PATH (or via
//! TKR_BIN), but the dependency surface stays tiny.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

/// Resolve the `tkr` binary. Honors `TKR_BIN` env override so tests and
/// non-PATH installs can point at a specific build.
fn tkr_bin() -> String {
    std::env::var("TKR_BIN").unwrap_or_else(|_| "tkr".to_string())
}

/// Run `tkr jobs list` with optional overrides and return its stdout.
///
/// `board`/`rpc_url` are passed through verbatim when set; otherwise the
/// `tkr` CLI uses its public-devnet defaults (DEFAULT_JOB_BOARD /
/// DEFAULT_RPC_URL). `limit` is forwarded as `--limit`.
pub fn list(board: Option<&str>, rpc_url: Option<&str>, limit: Option<usize>) -> Result<String> {
    let bin = tkr_bin();
    let mut cmd = Command::new(&bin);
    cmd.arg("job").arg("list");
    if let Some(b) = board {
        cmd.arg("--board").arg(b);
    }
    if let Some(r) = rpc_url {
        cmd.arg("--rpc-url").arg(r);
    }
    if let Some(n) = limit {
        cmd.arg("--limit").arg(n.to_string());
    }

    let out = cmd
        .output()
        .with_context(|| format!("spawn {bin}; ensure `tkr` is on PATH or set TKR_BIN"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "`{bin} job list` exit {}: {}",
            out.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All three sub-cases live in one test because cargo runs tests in
    /// parallel by default and TKR_BIN is process-global — separate
    /// `set_var` calls race and clobber each other. Inside one test we
    /// control the order, so the env stays consistent across sub-cases.
    #[test]
    fn list_argv_and_error_handling() {
        // Sub-case 1: overrides forwarded as flags.
        std::env::set_var("TKR_BIN", "/bin/echo");
        let out = list(Some("0xboard"), Some("http://rpc"), Some(7)).expect("echo ok");
        assert!(out.contains("job list"), "{out}");
        assert!(out.contains("--board 0xboard"), "{out}");
        assert!(out.contains("--rpc-url http://rpc"), "{out}");
        assert!(out.contains("--limit 7"), "{out}");

        // Sub-case 2: no overrides → no flags.
        let out = list(None, None, None).expect("echo ok");
        assert!(!out.contains("--board"), "{out}");
        assert!(!out.contains("--rpc-url"), "{out}");
        assert!(!out.contains("--limit"), "{out}");

        // Sub-case 3: nonzero exit surfaces as error.
        std::env::set_var("TKR_BIN", "/bin/false");
        let err = list(None, None, None).expect_err("false fails");
        assert!(format!("{err:#}").contains("exit"), "{err:#}");

        std::env::remove_var("TKR_BIN");
    }
}
