//! Read-only mesh-status MCP tool. Wraps the broker's
//! `/api/v1/mesh/status` endpoint by shelling out to curl — same pattern
//! as `jobs.rs` shells out to the jkr binary. Avoids pulling reqwest +
//! TLS into jkr-mcp for one HTTP GET.

use anyhow::{anyhow, Context, Result};
use std::process::Command;

const DEFAULT_HOST: &str = "https://tkr.prysm.sh";

/// Fetch broker status and render a compact text summary.
///
/// `host` is the scheme+authority (e.g. "https://tkr.prysm.sh" or
/// "http://127.0.0.1:4000"). Defaults to the public broker.
pub fn status(host: Option<&str>) -> Result<String> {
    let host = host.unwrap_or(DEFAULT_HOST).trim_end_matches('/');
    let url = format!("{host}/api/v1/mesh/status");

    let out = Command::new("curl")
        .args(["-sSf", "--max-time", "5", &url])
        .output()
        .with_context(|| "spawn curl; ensure curl is on PATH")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("curl {url} exit {}: {stderr}", out.status));
    }

    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("decode JSON from {url}"))?;
    Ok(render(host, &json))
}

fn render(host: &str, json: &serde_json::Value) -> String {
    let total_meshes = json
        .get("total_meshes")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total_connected = json
        .get("total_connected")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut out = format!(
        "broker {host}: {total_connected} peer(s) across {total_meshes} mesh(es)\n"
    );
    if total_meshes == 0 {
        out.push_str("  (no meshes — broker idle)\n");
        return out;
    }
    out.push_str(
        "\n  meshId                                    enrolled  connected\n  ────────────────────────────────────────────────────────────\n",
    );
    if let Some(meshes) = json.get("meshes").and_then(|v| v.as_array()) {
        for m in meshes {
            let id = m.get("meshId").and_then(|v| v.as_str()).unwrap_or("?");
            let enrolled = m.get("enrolled").and_then(|v| v.as_u64()).unwrap_or(0);
            let connected = m.get("connected").and_then(|v| v.as_u64()).unwrap_or(0);
            out.push_str(&format!("  {id:<41} {enrolled:>8}  {connected:>9}\n"));
        }
    }
    out
}
