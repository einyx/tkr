//! Symbol search + grep summary. Walks the file tree honoring `.gitignore`
//! via the `ignore` crate, then either runs a Rust outline pass and looks
//! for the symbol name, or runs a regex grep with per-file caps.

use anyhow::{anyhow, Context, Result};
use ignore::WalkBuilder;
use std::fs;
use std::path::{Path, PathBuf};

/// Find definitions of `symbol_name` under `root`. Returns one line per
/// hit: `kind name file:start-end`.
pub fn find_symbol(symbol_name: &str, root: &Path) -> Result<String> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", root.display()))?;
    let mut hits: Vec<String> = Vec::new();
    let mut files_scanned = 0usize;

    for entry in WalkBuilder::new(&root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        files_scanned += 1;
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Reuse the outline extractor so symbol shapes match what
        // tkr_outline_file would report.
        let symbols = match outline_internal(&bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for s in symbols {
            if s.name == symbol_name {
                let rel = path
                    .strip_prefix(&root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| path.display().to_string());
                hits.push(format!(
                    "{:<8} {:<32} {}:{}-{}",
                    s.kind, s.name, rel, s.start_line, s.end_line
                ));
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "find_symbol {:?} under {} ({} .rs files scanned, {} hits)\n",
        symbol_name,
        root.display(),
        files_scanned,
        hits.len()
    ));
    if hits.is_empty() {
        out.push_str("(no matches)\n");
    } else {
        for h in hits {
            out.push_str(&format!("  {h}\n"));
        }
    }
    Ok(out)
}

/// Grep summary: regex `pattern` over text files under `root`, grouped
/// by file. Caps total kept matches per file (default 3) and total
/// reported files (default 30).
pub fn grep_summary(
    pattern: &str,
    root: &Path,
    max_per_file: usize,
    max_files: usize,
) -> Result<String> {
    let re = regex::Regex::new(pattern).map_err(|e| anyhow!("invalid pattern: {e}"))?;
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", root.display()))?;

    struct FileHits {
        path: PathBuf,
        kept: Vec<(usize, String)>,
        total: usize,
    }
    let mut by_file: Vec<FileHits> = Vec::new();
    let mut files_scanned = 0usize;

    for entry in WalkBuilder::new(&root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        files_scanned += 1;
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        // Skip files that don't look like text.
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        let text = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let mut kept: Vec<(usize, String)> = Vec::new();
        let mut total = 0usize;
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                total += 1;
                if kept.len() < max_per_file {
                    let trimmed = if line.len() > 200 {
                        format!("{}…", &line[..200])
                    } else {
                        line.to_string()
                    };
                    kept.push((i + 1, trimmed));
                }
            }
        }
        if total > 0 {
            by_file.push(FileHits {
                path: path.to_path_buf(),
                kept,
                total,
            });
        }
        if by_file.len() >= max_files {
            break;
        }
    }

    by_file.sort_by(|a, b| b.total.cmp(&a.total));
    by_file.truncate(max_files);

    let mut out = String::new();
    let total_files_with_hits = by_file.len();
    let total_hits: usize = by_file.iter().map(|f| f.total).sum();
    out.push_str(&format!(
        "grep_summary {:?} under {} ({} files scanned, {} files with hits, {} total matches)\n",
        pattern,
        root.display(),
        files_scanned,
        total_files_with_hits,
        total_hits
    ));
    for f in &by_file {
        let rel = f
            .path
            .strip_prefix(&root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| f.path.display().to_string());
        let elided = f.total.saturating_sub(f.kept.len());
        out.push_str(&format!("\n{}: {} matches", rel, f.total));
        if elided > 0 {
            out.push_str(&format!(" (showing {})", f.kept.len()));
        }
        out.push('\n');
        for (lineno, line) in &f.kept {
            out.push_str(&format!("  L{lineno}: {line}\n"));
        }
    }
    Ok(out)
}

// ---------- shared helpers ----------

#[derive(Clone)]
struct OutlineSymbol {
    kind: &'static str,
    name: String,
    start_line: usize,
    end_line: usize,
}

fn outline_internal(bytes: &[u8]) -> Result<Vec<OutlineSymbol>> {
    use tree_sitter::{Parser, Query, QueryCursor};
    let mut parser = Parser::new();
    parser
        .set_language(tree_sitter_rust::language())
        .map_err(|e| anyhow!("set tree-sitter-rust: {e}"))?;
    let tree = parser
        .parse(bytes, None)
        .ok_or_else(|| anyhow!("parse failed"))?;
    let query_str = r#"
        (function_item name: (identifier) @fn.name) @fn
        (struct_item name: (type_identifier) @struct.name) @struct
        (enum_item name: (type_identifier) @enum.name) @enum
        (trait_item name: (type_identifier) @trait.name) @trait
        (mod_item name: (identifier) @mod.name) @mod
    "#;
    let query = Query::new(tree_sitter_rust::language(), query_str)
        .map_err(|e| anyhow!("compile query: {e}"))?;
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let capture_names = query.capture_names();
    for m in cursor.matches(&query, tree.root_node(), bytes) {
        let mut kind: Option<&'static str> = None;
        let mut start_line = 0usize;
        let mut end_line = 0usize;
        let mut name: Option<String> = None;
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize].as_str();
            if cap_name.contains('.') {
                if let Ok(s) = std::str::from_utf8(&bytes[cap.node.byte_range()]) {
                    name = Some(s.to_string());
                }
            } else {
                kind = Some(match cap_name {
                    "fn" => "fn",
                    "struct" => "struct",
                    "enum" => "enum",
                    "trait" => "trait",
                    "mod" => "mod",
                    _ => "?",
                });
                start_line = cap.node.start_position().row + 1;
                end_line = cap.node.end_position().row + 1;
            }
        }
        if let (Some(kind), Some(name)) = (kind, name) {
            out.push(OutlineSymbol {
                kind,
                name,
                start_line,
                end_line,
            });
        }
    }
    Ok(out)
}

