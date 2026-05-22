//! Stdio MCP server. Reads line-delimited JSON-RPC 2.0 requests on stdin,
//! writes responses on stdout. Errors are logged to stderr (Claude Code
//! shows the connection's stderr when the user opens MCP server details).

use anyhow::Result;
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::index_backed;
use crate::jobs;
use crate::mesh;
use crate::outline;
use crate::protocol::{
    initialize_result, text_result, tools_catalog, Request, Response,
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};
use crate::search;

/// Configured project root. Tools refuse paths that don't resolve under
/// this prefix. Defaults to the process CWD; override with TKR_MCP_ROOT.
fn project_root() -> PathBuf {
    if let Ok(v) = std::env::var("TKR_MCP_ROOT") {
        let p = PathBuf::from(v);
        if let Ok(c) = p.canonicalize() {
            return c;
        }
        return p;
    }
    std::env::current_dir()
        .and_then(|p| p.canonicalize().or(Ok(p)))
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolve `user_path` against `root` and confirm it stays under `root`
/// after symlink resolution. Returns the canonicalized absolute path.
fn confine(user_path: &str) -> Result<PathBuf> {
    let root = project_root();
    let raw = PathBuf::from(user_path);
    let absolute = if raw.is_absolute() {
        raw
    } else {
        root.join(&raw)
    };
    let canon = absolute
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("canonicalize {}: {e}", absolute.display()))?;
    if !canon.starts_with(&root) {
        return Err(anyhow::anyhow!(
            "path {} is outside project root {}",
            canon.display(),
            root.display()
        ));
    }
    Ok(canon)
}

pub struct Server;

impl Server {
    /// Run the stdio loop on stdin/stdout. Blocks until stdin closes.
    pub fn run() -> Result<()> {
        Self::run_io(
            BufReader::new(std::io::stdin().lock()),
            std::io::stdout().lock(),
        )
    }

    pub fn run_io<R: BufRead, W: Write>(mut reader: R, mut writer: W) -> Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(()); // EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response = handle_line(trimmed);
            if let Some(resp) = response {
                let bytes = serde_json::to_vec(&resp)?;
                writer.write_all(&bytes)?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
        }
    }
}

/// Handle one JSON-RPC line; returns `None` for notifications (no id).
pub fn handle_line(line: &str) -> Option<Response> {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(Response::err(
                Value::Null,
                PARSE_ERROR,
                format!("parse: {e}"),
            ));
        }
    };
    let id = req.id.clone();
    if id.is_none() {
        // Notification — handle the side effect (currently none) and
        // return None per JSON-RPC 2.0.
        return None;
    }
    let id = id.unwrap();

    if req.jsonrpc != "2.0" {
        return Some(Response::err(
            id,
            INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ));
    }

    match req.method.as_str() {
        "initialize" => Some(Response::ok(id, initialize_result())),
        "tools/list" => Some(Response::ok(id, tools_catalog())),
        "tools/call" => Some(handle_tools_call(id, &req.params)),
        // Other MCP methods we don't implement yet.
        _ => Some(Response::err(
            id,
            METHOD_NOT_FOUND,
            format!("method not found: {}", req.method),
        )),
    }
}

fn handle_tools_call(id: Value, params: &Value) -> Response {
    let name = match params.get("name").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return Response::err(id, INVALID_PARAMS, "missing tool name"),
    };
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    let result = match name {
        "tkr_outline_file" => call_outline(&args),
        "tkr_find_symbol" => call_find_symbol(&args),
        "tkr_grep_summary" => call_grep_summary(&args),
        "tkr_index_build" => call_index_build(&args),
        "tkr_index_watch" => call_index_watch(&args),
        "tkr_signature" => call_signature(&args),
        "tkr_read_smart" => call_read_smart(&args),
        "tkr_callers_of" => call_callers_of(&args),
        "tkr_callees_of" => call_callees_of(&args),
        "tkr_call_path" => call_call_path(&args),
        "tkr_jobs_list" => call_jobs_list(&args),
        "tkr_mesh_status" => call_mesh_status(&args),
        _ => return Response::err(id, METHOD_NOT_FOUND, format!("unknown tool: {name}")),
    };
    match result {
        Ok(text) => Response::ok(id, text_result(text)),
        Err(e) => Response::ok(
            id,
            serde_json::json!({
                "content": [{ "type": "text", "text": format!("[tkr-mcp error] {e}") }],
                "isError": true,
            }),
        ),
    }
}

fn call_outline(args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
    let confined = confine(path)?;
    outline::render_outline(&confined)
}

fn resolve_root(args: &Value) -> Result<PathBuf> {
    match args.get("root").and_then(|v| v.as_str()) {
        Some(r) => confine(r),
        None => Ok(project_root()),
    }
}

/// Run an index-backed query. If the index doesn't yet exist for this root,
/// build it inline before retrying — drops the "run tkr_index_build first"
/// friction that real transcripts showed was a tool-adoption killer.
///
/// On first-call: prepends a one-time `[tkr: built index in Ns]\n` line so
/// the agent can attribute the latency. Subsequent calls hit the cached
/// index normally and pay no overhead.
fn with_auto_index<F>(root: &PathBuf, f: F) -> Result<String>
where
    F: Fn(&Path) -> Result<Option<String>>,
{
    match f(root)? {
        Some(out) => Ok(out),
        None => {
            let started = std::time::Instant::now();
            let _ = index_backed::build(root)?;
            let elapsed_ms = started.elapsed().as_millis();
            let out = f(root)?.ok_or_else(|| {
                anyhow::anyhow!("internal: index built but query still returned None")
            })?;
            Ok(format!("[tkr: built index in {elapsed_ms}ms]\n{out}"))
        }
    }
}

fn call_find_symbol(args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?;
    let root = resolve_root(args)?;
    // Prefer the persistent index when one exists; fall back to the
    // stateless scan otherwise so existing users see no behavior change.
    if let Some(out) = index_backed::try_find_symbol(name, &root)? {
        return Ok(out);
    }
    search::find_symbol(name, &root)
}

fn call_index_build(args: &Value) -> Result<String> {
    let root = resolve_root(args)?;
    index_backed::build(&root)
}

fn call_index_watch(args: &Value) -> Result<String> {
    let root = resolve_root(args)?;
    index_backed::watch_start(&root)
}

fn call_signature(args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?
        .to_string();
    let root = resolve_root(args)?;
    with_auto_index(&root, |r| index_backed::try_signature(&name, r))
}

fn call_callers_of(args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?
        .to_string();
    let root = resolve_root(args)?;
    with_auto_index(&root, |r| index_backed::try_callers_of(&name, r))
}

fn call_callees_of(args: &Value) -> Result<String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'name'"))?
        .to_string();
    let root = resolve_root(args)?;
    with_auto_index(&root, |r| index_backed::try_callees_of(&name, r))
}

fn call_call_path(args: &Value) -> Result<String> {
    let from = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'from'"))?
        .to_string();
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'to'"))?
        .to_string();
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(6);
    let root = resolve_root(args)?;
    with_auto_index(&root, |r| index_backed::try_call_path(&from, &to, max_depth, r))
}

fn call_read_smart(args: &Value) -> Result<String> {
    let question = args
        .get("question")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'question'"))?
        .to_string();
    let root = resolve_root(args)?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(8);
    let verbose = args
        .get("verbose")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    with_auto_index(&root, |r| {
        index_backed::try_read_smart(&question, r, limit, verbose)
    })
}

fn call_mesh_status(_args: &Value) -> Result<String> {
    // Same prompt-injection concern as call_jobs_list: don't let the LLM
    // pick the host. Operators override via TKR_MESH_HOST.
    let host = std::env::var("TKR_MESH_HOST").ok();
    mesh::status(host.as_deref())
}

fn call_jobs_list(args: &Value) -> Result<String> {
    // board/rpc_url are intentionally NOT exposed in the MCP schema: a
    // prompt-injected LLM could otherwise be steered to point them at
    // internal services (cloud metadata, localhost) and exfiltrate the
    // upstream's response/stderr through the tool result. Operators set
    // these via env vars at server start; env is trusted, LLM args are not.
    let board = std::env::var("TKR_JOB_BOARD").ok();
    let rpc_url = std::env::var("TKR_JOB_RPC_URL").ok();
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);
    jobs::list(board.as_deref(), rpc_url.as_deref(), limit)
}

fn call_grep_summary(args: &Value) -> Result<String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'pattern'"))?;
    let root = resolve_root(args)?;
    let max_per_file = args
        .get("max_per_file")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(3);
    let max_files = args
        .get("max_files")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(30);
    search::grep_summary(pattern, &root, max_per_file, max_files)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(line: &str) -> Value {
        let r = handle_line(line).expect("response");
        serde_json::to_value(&r).unwrap()
    }

    #[test]
    fn initialize_returns_capabilities() {
        let v = rpc(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], "tkr-mcp");
    }

    #[test]
    fn tools_list_includes_outline() {
        let v = rpc(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = v["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"tkr_outline_file"));
        assert!(names.contains(&"tkr_find_symbol"));
        assert!(names.contains(&"tkr_grep_summary"));
        assert!(names.contains(&"tkr_jobs_list"));
        assert!(names.contains(&"tkr_mesh_status"));
    }

    #[test]
    fn unknown_method_returns_error() {
        let v = rpc(r#"{"jsonrpc":"2.0","id":3,"method":"banana"}"#);
        assert_eq!(v["error"]["code"], -32601);
    }

    #[test]
    fn parse_error_for_invalid_json() {
        let v = rpc("not-json");
        assert_eq!(v["error"]["code"], -32700);
    }

    #[test]
    fn with_auto_index_builds_on_first_call() {
        // Fresh temp repo: no .tkr/index.sqlite exists. After one call to
        // an index-requiring tool, the index file should exist AND the
        // response should carry the `[tkr: built index in Nms]` notice.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("auto.rs");
        std::fs::write(
            &src,
            "fn target() -> u32 { 0 }\nfn caller() { target(); }\n",
        )
        .unwrap();
        let root_buf = dir.path().to_path_buf();

        // No index file before the call.
        assert!(!root_buf.join(".tkr").join("index.sqlite").exists());

        let out = with_auto_index(&root_buf, |r| {
            index_backed::try_callers_of("target", r)
        })
        .unwrap();

        assert!(
            out.starts_with("[tkr: built index in"),
            "expected build notice, got:\n{out}"
        );
        assert!(
            out.contains("caller"),
            "expected the query result after the notice:\n{out}"
        );
        // Second call must NOT re-build (no notice).
        let out2 = with_auto_index(&root_buf, |r| {
            index_backed::try_callers_of("target", r)
        })
        .unwrap();
        assert!(
            !out2.starts_with("[tkr:"),
            "second call should hit cached index (no notice), got:\n{out2}"
        );
    }

    #[test]
    fn notifications_produce_no_response() {
        // No `id` field = notification.
        assert!(handle_line(r#"{"jsonrpc":"2.0","method":"initialize"}"#).is_none());
    }

    #[test]
    fn tools_call_outline_missing_path_is_error() {
        let v = rpc(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"tkr_outline_file","arguments":{}}}"#,
        );
        // Tool errors come back as a successful JSON-RPC reply with isError=true.
        let content = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(content.contains("missing 'path'"), "{content}");
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn unknown_tool_returns_method_not_found() {
        let v = rpc(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"banana","arguments":{}}}"#,
        );
        assert_eq!(v["error"]["code"], -32601);
    }

}
