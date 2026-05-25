//! TOON (Token-Oriented Object Notation) rendering for tool output.
//!
//! TOON is a compact alternative to JSON / pretty text for tabular data:
//! the column names are declared once in a header, each row is a single
//! comma-separated line. Designed so an LLM consuming the output spends
//! fewer tokens parsing structural noise.
//!
//! Example:
//!   ```text
//!   find_symbol "Server" -- 2 hits
//!   hits[2]{kind,name,path,start,end}:
//!     function,Server,crates/tkr-mcp/src/server.rs,30,50
//!     function,Server,crates/tkr-server/src/lib.rs,15,100
//!   ```
//!
//! Toggle with `TKR_TOON=1` in the MCP server's env. Off by default — we
//! ship behavior change behind a flag until users opt in.

use std::fmt::Write;

pub fn enabled() -> bool {
    matches!(
        std::env::var("TKR_TOON").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes")
    )
}

/// Render a labeled table. `cols` are header names; `rows` are values in
/// the same order. Commas/newlines in cell values are escaped with `\`.
pub fn table(label: &str, cols: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{label}");
    let _ = write!(out, "rows[{}]{{", rows.len());
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(c);
    }
    out.push_str("}:\n");
    for row in rows {
        out.push_str("  ");
        for (i, v) in row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&escape_cell(v));
        }
        out.push('\n');
    }
    out
}

fn escape_cell(s: &str) -> String {
    if s.contains(',') || s.contains('\n') || s.contains('\\') {
        let mut e = String::with_capacity(s.len() + 4);
        for c in s.chars() {
            match c {
                ',' => e.push_str("\\,"),
                '\n' => e.push_str("\\n"),
                '\\' => e.push_str("\\\\"),
                _ => e.push(c),
            }
        }
        e
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_table() {
        let out = table(
            "find_symbol \"x\" -- 2 hits",
            &["kind", "name", "path", "start", "end"],
            &[
                vec![
                    "function".into(),
                    "x".into(),
                    "a/b.rs".into(),
                    "10".into(),
                    "20".into(),
                ],
                vec![
                    "method".into(),
                    "x".into(),
                    "c/d.rs".into(),
                    "5".into(),
                    "8".into(),
                ],
            ],
        );
        assert!(out.contains("rows[2]{kind,name,path,start,end}:"));
        assert!(out.contains("function,x,a/b.rs,10,20"));
    }

    #[test]
    fn escapes_commas() {
        assert_eq!(escape_cell("a,b"), "a\\,b");
        assert_eq!(escape_cell("plain"), "plain");
    }
}
