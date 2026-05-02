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
                "description": "Return a structured outline (symbol kind + name + line range) for a source file instead of its full contents. Use this when you only need to know what a file contains, not the implementation details. Currently supports Rust; falls back to a header-line summary for other languages.",
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
                "description": "Find definitions of a symbol (function, struct, type, etc.) under a directory. Returns file:line locations + the symbol kind. Drastically smaller token cost than recursive Grep when you know what you're looking for.",
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
                "description": "Like grep -r, but returns matches grouped by file with per-file caps. Use instead of native Grep when expecting many matches across many files.",
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
