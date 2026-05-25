//! End-to-end token-cost bench for every index-backed MCP tool.
//!
//! Runs each tool on a real repo with a fixed query set and reports the
//! byte count of the response. Byte count is used as a token proxy —
//! Claude's tokenizer averages ~3.5 chars/token on code-shaped text, but
//! bytes are honest for *comparative* measurements without pulling a
//! tokenizer dependency.
//!
//! Usage:
//!   cargo run -p jkr-mcp --release --example bench_token_cost -- <repo_root>
//!
//! Defaults to `.` if no arg given. Prints a per-tool table, then a total
//! "session budget" assuming a representative mix of calls.

use std::path::PathBuf;
use jkr_mcp::{index_backed, outline};

/// Representative queries by tool. Mirrors what a real agent session looks
/// like — a handful of symbol lookups + structural questions, not exhaustive
/// coverage of the index.
struct Queries {
    outline_files: Vec<&'static str>,
    find_symbols: Vec<&'static str>,
    signatures: Vec<&'static str>,
    read_smarts: Vec<&'static str>,
    callers: Vec<&'static str>,
    callees: Vec<&'static str>,
    call_paths: Vec<(&'static str, &'static str)>,
}

fn queries() -> Queries {
    Queries {
        outline_files: vec![
            "crates/jkr-mcp/src/index_backed.rs",
            "crates/jkr-mcp/src/server.rs",
            "crates/jkr-sandbox/src/macos.rs",
        ],
        find_symbols: vec!["run_sandboxed", "try_find_symbol", "build_profile"],
        signatures: vec!["run_sandboxed", "try_call_path"],
        read_smarts: vec![
            "where is the sandbox profile built",
            "how does the index get refreshed",
            "call graph traversal",
        ],
        callers: vec!["run_sandboxed", "build_profile"],
        callees: vec!["try_call_path", "build_profile"],
        call_paths: vec![("run", "build_profile"), ("handle_tools_call", "build")],
    }
}

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let root = root.canonicalize()?;
    eprintln!("== building index for {} ==", root.display());
    let built = index_backed::build(&root)?;
    eprint!("{built}");

    let q = queries();
    let mut grand_total = 0usize;
    let mut per_tool: Vec<(&str, usize, usize)> = Vec::new(); // (tool, calls, bytes)

    let mut record = |tool: &'static str, bytes: usize, calls: usize| {
        per_tool.push((tool, calls, bytes));
        grand_total += bytes;
    };

    // outline_file
    let mut bytes = 0usize;
    for f in &q.outline_files {
        let p = root.join(f);
        if !p.exists() {
            eprintln!("skip outline {} (not in repo)", f);
            continue;
        }
        bytes += outline::render_outline(&p)?.len();
    }
    record("jkr_outline_file", bytes, q.outline_files.len());

    // find_symbol
    let mut bytes = 0usize;
    for s in &q.find_symbols {
        if let Some(out) = index_backed::try_find_symbol(s, &root)? {
            bytes += out.len();
        }
    }
    record("jkr_find_symbol", bytes, q.find_symbols.len());

    // signature
    let mut bytes = 0usize;
    for s in &q.signatures {
        if let Some(out) = index_backed::try_signature(s, &root)? {
            bytes += out.len();
        }
    }
    record("jkr_signature", bytes, q.signatures.len());

    // read_smart
    let mut bytes = 0usize;
    for question in &q.read_smarts {
        if let Some(out) = index_backed::try_read_smart(question, &root, 8, false)? {
            bytes += out.len();
        }
    }
    record("jkr_read_smart", bytes, q.read_smarts.len());

    // callers_of
    let mut bytes = 0usize;
    for s in &q.callers {
        if let Some(out) = index_backed::try_callers_of(s, &root)? {
            bytes += out.len();
        }
    }
    record("jkr_callers_of", bytes, q.callers.len());

    // callees_of
    let mut bytes = 0usize;
    for s in &q.callees {
        if let Some(out) = index_backed::try_callees_of(s, &root)? {
            bytes += out.len();
        }
    }
    record("jkr_callees_of", bytes, q.callees.len());

    // call_path
    let mut bytes = 0usize;
    for (from, to) in &q.call_paths {
        if let Some(out) = index_backed::try_call_path(from, to, 6, &root)? {
            bytes += out.len();
        }
    }
    record("jkr_call_path", bytes, q.call_paths.len());

    println!();
    println!("{:<22} {:>6} {:>10} {:>10}", "tool", "calls", "bytes", "B/call");
    println!("{}", "-".repeat(50));
    for (tool, calls, bytes) in &per_tool {
        let per = if *calls > 0 { *bytes / *calls } else { 0 };
        println!("{:<22} {:>6} {:>10} {:>10}", tool, calls, bytes, per);
    }
    println!("{}", "-".repeat(50));
    println!(
        "{:<22} {:>6} {:>10}",
        "TOTAL",
        per_tool.iter().map(|(_, c, _)| c).sum::<usize>(),
        grand_total
    );

    // Token estimate — Claude's tokenizer averages ~3.5 chars/token on
    // code-shaped text. Hand-wavy but useful for comparing to context budgets.
    println!();
    println!(
        "~{:.0} tokens at 3.5 chars/token (Claude's avg on code)",
        grand_total as f64 / 3.5
    );

    Ok(())
}
