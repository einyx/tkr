//! Parse a file with tree-sitter and write its symbols into the index.

use anyhow::{anyhow, Context, Result};
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::SystemTime;
use tree_sitter::{Parser, Query, QueryCursor};

use crate::lang::{self, LangSpec};
use crate::IndexDb;

pub struct IndexStats {
    pub symbols: usize,
    pub skipped_unchanged: bool,
}

impl IndexDb {
    /// Index a single file. No-op if the file's content hash already matches.
    pub fn index_file(&mut self, path: &Path) -> Result<IndexStats> {
        let rel = path.to_string_lossy().to_string();
        let spec = match lang::detect(&rel) {
            Some(s) => s,
            None => {
                return Ok(IndexStats {
                    symbols: 0,
                    skipped_unchanged: false,
                })
            }
        };

        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let hash = hex::encode(Sha256::digest(&bytes));

        if self.is_fresh(&rel, &hash)? {
            return Ok(IndexStats {
                symbols: 0,
                skipped_unchanged: true,
            });
        }

        let symbols = extract_symbols(&bytes, &spec)?;
        let calls = extract_calls(&bytes, &spec)?;

        let mtime_ns = fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let tx = self.conn_mut().transaction()?;
        // Replace existing entry: cascade deletes wipe old symbols/refs.
        tx.execute("DELETE FROM files WHERE path = ?1", params![rel])?;
        tx.execute(
            "INSERT INTO files(path, lang, content_hash, mtime_ns, indexed_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![rel, spec.name, hash, mtime_ns, now],
        )?;
        let file_id = tx.last_insert_rowid();

        let n = symbols.len();
        for s in &symbols {
            tx.execute(
                "INSERT INTO symbols(file_id, parent_id, name, kind, signature, line_start, line_end, docstring)
                 VALUES(?1, NULL, ?2, ?3, ?4, ?5, ?6, NULL)",
                params![file_id, s.name, s.kind, s.signature, s.line_start, s.line_end],
            )?;
        }

        // Resolve each call to its enclosing symbol via SQL: deepest range
        // containing the call's line wins. Skip calls outside any symbol
        // (e.g. top-level code).
        for c in &calls {
            let from_id: Option<i64> = tx
                .query_row(
                    "SELECT id FROM symbols
                     WHERE file_id = ?1 AND line_start <= ?2 AND line_end >= ?2
                     ORDER BY (line_end - line_start) ASC
                     LIMIT 1",
                    params![file_id, c.line],
                    |r| r.get(0),
                )
                .ok();
            if let Some(fid) = from_id {
                tx.execute(
                    "INSERT INTO refs(from_symbol_id, to_name, line, kind)
                     VALUES(?1, ?2, ?3, 'call')",
                    params![fid, c.callee, c.line],
                )?;
            }
        }
        tx.commit()?;

        Ok(IndexStats {
            symbols: n,
            skipped_unchanged: false,
        })
    }

    fn conn_mut(&mut self) -> &mut rusqlite::Connection {
        &mut self.conn
    }
}

struct ExtractedSymbol {
    kind: &'static str,
    name: String,
    signature: String,
    line_start: i64,
    line_end: i64,
}

fn extract_symbols(source: &[u8], spec: &LangSpec) -> Result<Vec<ExtractedSymbol>> {
    let language = (spec.language)();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| anyhow!("set language {}: {e}", spec.name))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("parse failed for {}", spec.name))?;
    let query = Query::new(&language, spec.query)
        .map_err(|e| anyhow!("compile query for {}: {e}", spec.name))?;

    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(&query, tree.root_node(), source) {
        let mut kind: Option<&'static str> = None;
        let mut name: Option<String> = None;
        let mut start_line = 0i64;
        let mut end_line = 0i64;
        let mut signature = String::new();
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize];
            let node_bytes = &source[cap.node.byte_range()];
            if cap_name.contains('.') {
                // .name sub-capture
                if let Ok(s) = std::str::from_utf8(node_bytes) {
                    name = Some(s.to_string());
                }
            } else {
                kind = Some(lang::canonical_kind(cap_name));
                start_line = (cap.node.start_position().row + 1) as i64;
                end_line = (cap.node.end_position().row + 1) as i64;
                // Signature = first non-empty line of the captured node.
                if let Ok(s) = std::str::from_utf8(node_bytes) {
                    if let Some(line) = s.lines().find(|l| !l.trim().is_empty()) {
                        signature = truncate(line.trim(), 200);
                    }
                }
            }
        }
        if let (Some(k), Some(n)) = (kind, name) {
            out.push(ExtractedSymbol {
                kind: k,
                name: n,
                signature,
                line_start: start_line,
                line_end: end_line,
            });
        }
    }
    out.sort_by_key(|s| s.line_start);
    Ok(out)
}

struct ExtractedCall {
    callee: String,
    line: i64,
}

fn extract_calls(source: &[u8], spec: &LangSpec) -> Result<Vec<ExtractedCall>> {
    let language = (spec.language)();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| anyhow!("set language {}: {e}", spec.name))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("parse failed for {}", spec.name))?;
    let query = Query::new(&language, spec.calls_query)
        .map_err(|e| anyhow!("compile calls query for {}: {e}", spec.name))?;

    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    for m in cursor.matches(&query, tree.root_node(), source) {
        let mut callee: Option<String> = None;
        let mut line: i64 = 0;
        for cap in m.captures {
            let cap_name = capture_names[cap.index as usize];
            if cap_name.ends_with(".name") {
                if let Ok(s) = std::str::from_utf8(&source[cap.node.byte_range()]) {
                    callee = Some(s.to_string());
                }
            } else {
                line = (cap.node.start_position().row + 1) as i64;
            }
        }
        if let Some(c) = callee {
            out.push(ExtractedCall { callee: c, line });
        }
    }
    Ok(out)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('…');
        t
    }
}
