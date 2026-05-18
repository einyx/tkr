//! tkr-mcp — MCP (Model Context Protocol) server.
//!
//! Speaks JSON-RPC 2.0 over stdio. Exposes a small set of code-intelligence
//! tools that return structured summaries instead of raw file contents:
//!
//!   tkr_outline_file  — symbols + line numbers (functions, structs, etc.)
//!   tkr_find_symbol   — locate definitions of a symbol across the tree
//!   tkr_grep_summary  — grep with per-file aggregation + line caps
//!
//! Why: Claude Code's native `Read` returns the entire file; `Grep` returns
//! every match line. For large files / wide patterns those are 50-150K
//! tokens. Routing through these MCP tools instead drops most of that.
//!
//! Public API:
//!   * [`Server`]      — owns the stdio loop. Use [`Server::run`] from a
//!                       binary's main.
//!   * [`handle_call`] — pure function from a `tools/call` payload to a
//!                       JSON result. Useful for testing.

pub mod index_backed;
pub mod toon;
pub mod jobs;
pub mod mesh;
pub mod outline;
pub mod protocol;
pub mod search;
pub mod server;

pub use server::Server;
