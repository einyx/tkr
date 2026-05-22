//! Minimal MCP wire types. We support enough of the spec to handle
//! `initialize`, `tools/list`, and `tools/call` from Claude Code; other
//! requests are answered with a `MethodNotFound` error.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none", flatten)]
    pub body: Option<ResponseBody>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponseBody {
    Result { result: Value },
    Error { error: ErrorObject },
}

#[derive(Debug, Serialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            body: Some(ResponseBody::Result { result }),
        }
    }
    pub fn err(id: Value, code: i32, message: impl Into<String>) -> Self {
        Response {
            jsonrpc: "2.0",
            id,
            body: Some(ResponseBody::Error {
                error: ErrorObject {
                    code,
                    message: message.into(),
                    data: None,
                },
            }),
        }
    }
}

// JSON-RPC standard error codes used by the server.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

/// MCP `initialize` response shape.
pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "tkr-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

/// Static catalog of tools exposed to the LLM. Schema is JSON Schema
/// for the input arguments.
pub fn tools_catalog() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "tkr_outline_file",
                "description": "USE BEFORE `Read` on any source file >200 lines. Returns symbol kind + name + line range (no bodies) — typically 5-15% the byte cost of reading the file. Workflow: outline first to find the right line range, THEN native Read with offset/limit for the body. Supports rust/python/go/ts/js/java/c/c++/ruby. Errors helpfully if you pass a directory (use tkr_grep_summary or tkr_find_symbol instead).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path to the file."
                        }
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "tkr_find_symbol",
                "description": "USE INSTEAD OF `Grep` when you know the exact symbol name (function/struct/type/method). One indexed lookup returns every definition site in the repo at <100B per response. Native Grep on the same name typically returns 50-500× more bytes because it matches every call site, comment, and string literal too. Falls back to a stateless scan if no index exists, so it works on any repo without setup.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Symbol name to find. Exact match by default."
                        },
                        "root": {
                            "type": "string",
                            "description": "Directory to search under. Defaults to CWD."
                        }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "tkr_grep_summary",
                "description": "USE INSTEAD OF `Grep` for any pattern likely to hit >10 files. Returns matches grouped by file with per-file caps (default 3 matches/file, 30 files total) — bounded output even when the pattern matches thousands of lines. Native Grep dumps everything; this gives you a navigable digest. For exact symbol-name lookups prefer tkr_find_symbol (even cheaper).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern to search for."
                        },
                        "root": {
                            "type": "string",
                            "description": "Directory to search under. Defaults to CWD."
                        },
                        "max_per_file": {
                            "type": "integer",
                            "description": "Max matches kept per file. Default 3.",
                            "minimum": 1,
                            "maximum": 50
                        },
                        "max_files": {
                            "type": "integer",
                            "description": "Max files kept in the output. Default 30.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    },
                    "required": ["pattern"]
                }
            },
            {
                "name": "tkr_jobs_list",
                "description": "List jobs on the tkr JobBoard contract: id, status (Open/Taken/Completed/Accepted/Cancelled/TimedOut), reward in wei, deadline, and a short preview of the spec. Read-only — no key required, no on-chain writes. Targets the tkr devnet board by default; operators can override via the TKR_JOB_BOARD / TKR_JOB_RPC_URL env vars on the MCP server.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Max jobs returned. Default 50.",
                            "minimum": 1,
                            "maximum": 500
                        }
                    }
                }
            },
            {
                "name": "tkr_callers_of",
                "description": "USE INSTEAD OF `Grep \"\\bfoo\\(\"` when answering 'where is X called?'. Returns every call site of a symbol by name from the indexed refs table, grouped per caller with line lists. Name resolution is unqualified (matches any `foo()` regardless of module/receiver). First call auto-builds the index (~1-5s on a normal repo); subsequent calls are instant.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "root": { "type": "string" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "tkr_callees_of",
                "description": "USE INSTEAD OF reading a function's body to figure out 'what does X actually do?'. Returns the list of unresolved callees referenced inside the symbol, deduped with call-site line lists. Reading the body costs N×line-bytes; this is ~50B regardless of body size. First call auto-builds the index (~1-5s on a normal repo); subsequent calls are instant.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "root": { "type": "string" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "tkr_call_path",
                "description": "USE INSTEAD OF walking callees by hand for 'does X eventually reach Y?' questions. Shortest call-path between two symbols via BFS, bounded depth, cycle-safe. One call replaces ~depth × callees_of invocations. Returns the chain `from -> A -> B -> to` with per-hop lines, or 'no path'. First call auto-builds the index (~1-5s on a normal repo); subsequent calls are instant.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "Caller name — search starts here." },
                        "to":   { "type": "string", "description": "Target name — search ends when any symbol with this name is reached." },
                        "max_depth": {
                            "type": "integer",
                            "description": "Hop cap (default 6). Larger = more chance of finding deep paths, but slower and noisier on dense graphs.",
                            "minimum": 1,
                            "maximum": 32
                        },
                        "root": { "type": "string" }
                    },
                    "required": ["from", "to"]
                }
            },
            {
                "name": "tkr_signature",
                "description": "USE INSTEAD OF `Read` on a file just to see a function's signature. Returns kind + name + the one-line declaration + file:line. ~50B vs reading 500-5000B of file just to find the signature. Pairs with tkr_read_smart (which gives location, then this gives shape). First call auto-builds the index (~1-5s on a normal repo); subsequent calls are instant.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Symbol name." },
                        "root": { "type": "string", "description": "Repo root. Defaults to CWD." }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "tkr_read_smart",
                "description": "USE FIRST for 'where is X done in this codebase?' questions instead of guessing files. FTS-ranked symbol search via natural-language query. Returns the top-K best-matched symbols with kind/name/location — no bodies, no signatures by default. Then drill in: tkr_signature for shape, native Read with the line range for body. Pass verbose=true to inline signatures when you specifically need them. First call auto-builds the index (~1-5s on a normal repo); subsequent calls are instant.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "Free-form question or keywords." },
                        "root": { "type": "string", "description": "Repo root. Defaults to CWD." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Max symbols returned. Default 8." },
                        "verbose": { "type": "boolean", "description": "Include the per-symbol signature line. Default false — terse output saves ~40% on real repos. Set true when you specifically need the shape of each hit, not just its location." }
                    },
                    "required": ["question"]
                }
            },
            {
                "name": "tkr_index_watch",
                "description": "Start a background file watcher for a repo. After this, file edits trigger automatic incremental re-indexing (debounced 500ms) — no need to call tkr_index_build again. Idempotent; safe to call multiple times for the same root. The watcher persists for the lifetime of the MCP server process.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "root": { "type": "string", "description": "Repo root. Defaults to CWD." }
                    }
                }
            },
            {
                "name": "tkr_index_build",
                "description": "Build or refresh the persistent code index for a repo. Walks gitignore-aware and re-parses only files whose content changed. Once built, tkr_find_symbol queries the index (millisecond lookups) instead of scanning the tree. Safe to call repeatedly.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "root": {
                            "type": "string",
                            "description": "Repo root. Defaults to CWD."
                        }
                    }
                }
            },
            {
                "name": "tkr_mesh_status",
                "description": "Show live mesh broker status — how many peers are connected to each mesh on the tkr broker. Read-only HTTP GET against /api/v1/mesh/status. Targets the public broker at https://tkr.prysm.sh by default; operators can override via the TKR_MESH_HOST env var on the MCP server.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

/// Wrap a plain text payload as an MCP `tools/call` result.
pub fn text_result(text: impl Into<String>) -> Value {
    serde_json::json!({
        "content": [
            { "type": "text", "text": text.into() }
        ]
    })
}
