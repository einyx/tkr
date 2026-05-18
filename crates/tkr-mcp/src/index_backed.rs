//! Index-backed tool implementations. When `<repo>/.tkr/index.sqlite` exists,
//! these return Some(rendered) and the server uses them; otherwise None and
//! the server falls back to the stateless scanners in `search.rs` / `outline.rs`.

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rusqlite::params;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tkr_index::watch::{self, WatcherHandle};
use tkr_index::IndexDb;

use crate::toon;

fn index_path(root: &Path) -> PathBuf {
    root.join(".tkr").join("index.sqlite")
}

/// One watcher per repo root, held alive for the lifetime of the MCP server.
fn watchers() -> &'static Mutex<HashMap<PathBuf, WatcherHandle>> {
    static W: OnceLock<Mutex<HashMap<PathBuf, WatcherHandle>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start (or report) a watcher for `root`. Idempotent.
pub fn watch_start(root: &Path) -> Result<String> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", root.display()))?;
    let mut map = watchers().lock().expect("watchers mutex");
    if map.contains_key(&root) {
        return Ok(format!("tkr_index_watch: already running for {}\n", root.display()));
    }
    let handle = watch::start(&root)?;
    map.insert(root.clone(), handle);
    Ok(format!(
        "tkr_index_watch: started for {} -- file edits now auto-reindex (debounced 500ms)\n",
        root.display()
    ))
}

/// Build (or refresh) the index for `root`. Walks gitignore-aware,
/// re-parses only files whose content hash changed.
pub fn build(root: &Path) -> Result<String> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", root.display()))?;
    let mut db = IndexDb::open(&root)?;
    let mut total = 0usize;
    let mut reindexed = 0usize;
    let mut symbols = 0usize;
    for entry in WalkBuilder::new(&root)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if tkr_index_lang_supported(path) {
            total += 1;
            let stats = db.index_file(path)?;
            if !stats.skipped_unchanged {
                reindexed += 1;
                symbols += stats.symbols;
            }
        }
    }
    Ok(format!(
        "tkr_index_build {} -- {} indexable files, {} re-indexed, {} new symbols\n",
        root.display(),
        total,
        reindexed,
        symbols
    ))
}

fn tkr_index_lang_supported(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|s| s.to_str()),
        Some(
            "rs" | "py" | "go" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
                | "java" | "c" | "h" | "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "rb"
        )
    )
}

/// Try to answer `find_symbol` from the index. Returns Ok(None) when no
/// index DB exists at the repo root — caller should fall back.
pub fn try_find_symbol(name: &str, root: &Path) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;
    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, f.path, s.line_start, s.line_end
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.name = ?1
         ORDER BY f.path, s.line_start",
    )?;
    let rows: Vec<(String, String, String, i64, i64)> = stmt
        .query_map(params![name], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if toon::enabled() {
        let label = format!("find_symbol {:?} -- {} hits", name, rows.len());
        if rows.is_empty() {
            return Ok(Some(format!("{label}\n(no matches)\n")));
        }
        let toon_rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(kind, sname, path, a, b)| {
                vec![kind, sname, path, a.to_string(), b.to_string()]
            })
            .collect();
        return Ok(Some(toon::table(
            &label,
            &["kind", "name", "path", "start", "end"],
            &toon_rows,
        )));
    }
    let mut out = String::new();
    out.push_str(&format!(
        "find_symbol {:?} via index ({} hits)\n",
        name,
        rows.len()
    ));
    if rows.is_empty() {
        out.push_str("(no matches — try tkr_index_build if the repo changed)\n");
    } else {
        for (kind, sname, path, a, b) in rows {
            out.push_str(&format!("  {kind:<8} {sname:<32} {path}:{a}-{b}\n"));
        }
    }
    Ok(Some(out))
}

/// Signature lookup: name → kind, signature line, location. Tiny output.
/// Returns Ok(None) if no index exists.
pub fn try_signature(name: &str, root: &Path) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;
    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, coalesce(s.signature,''), f.path, s.line_start, s.line_end
         FROM symbols s JOIN files f ON s.file_id = f.id
         WHERE s.name = ?1
         ORDER BY f.path, s.line_start",
    )?;
    let rows: Vec<(String, String, String, String, i64, i64)> = stmt
        .query_map(params![name], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut out = String::new();
    out.push_str(&format!("signature {:?} ({} hits)\n", name, rows.len()));
    if rows.is_empty() {
        out.push_str("(no matches — try tkr_index_build if the repo changed)\n");
    } else {
        for (kind, sname, sig, path, a, b) in rows {
            out.push_str(&format!("  {kind} {sname}  {path}:{a}-{b}\n"));
            if !sig.is_empty() {
                out.push_str(&format!("    {sig}\n"));
            }
        }
    }
    Ok(Some(out))
}

/// "Who calls X?" — find every symbol that contains a call site to `name`.
pub fn try_callers_of(name: &str, root: &Path) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;
    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, f.path, r.line
         FROM refs r
         JOIN symbols s ON s.id = r.from_symbol_id
         JOIN files   f ON f.id = s.file_id
         WHERE r.to_name = ?1
         ORDER BY f.path, r.line",
    )?;
    let rows: Vec<(String, String, String, i64)> = stmt
        .query_map(params![name], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if toon::enabled() {
        let label = format!("callers_of {:?} -- {} call sites", name, rows.len());
        if rows.is_empty() {
            return Ok(Some(format!("{label}\n(none)\n")));
        }
        let toon_rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(kind, sname, path, line)| vec![kind, sname, path, line.to_string()])
            .collect();
        return Ok(Some(toon::table(
            &label,
            &["kind", "from", "path", "line"],
            &toon_rows,
        )));
    }
    let mut out = String::new();
    out.push_str(&format!("callers_of {:?} ({} call sites)\n", name, rows.len()));
    if rows.is_empty() {
        out.push_str("(no callers found — name resolution is unqualified, so this means literally no call sites use this identifier)\n");
    } else {
        for (kind, sname, path, line) in rows {
            out.push_str(&format!("  {kind} {sname:<32} {path}:{line}\n"));
        }
    }
    Ok(Some(out))
}

/// "What does X call?" — find every callee referenced inside the symbol(s)
/// named `name`. May return multiple if name is overloaded across files.
pub fn try_callees_of(name: &str, root: &Path) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;
    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, f.path, s.line_start, r.to_name, r.line
         FROM symbols s
         JOIN files   f ON f.id = s.file_id
         LEFT JOIN refs r ON r.from_symbol_id = s.id
         WHERE s.name = ?1
         ORDER BY f.path, s.line_start, r.line",
    )?;
    let rows: Vec<(String, String, String, i64, Option<String>, Option<i64>)> = stmt
        .query_map(params![name], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut out = String::new();
    out.push_str(&format!("callees_of {:?}\n", name));
    if rows.is_empty() {
        out.push_str("(symbol not found in index)\n");
        return Ok(Some(out));
    }
    // Group by symbol location.
    let mut current: Option<(String, String, i64)> = None;
    let mut any_call = false;
    for (kind, sname, path, lstart, callee, line) in rows {
        let key = (path.clone(), sname.clone(), lstart);
        if current.as_ref() != Some(&key) {
            out.push_str(&format!("\n{kind} {sname}  {path}:{lstart}\n"));
            current = Some(key);
        }
        if let (Some(c), Some(l)) = (callee, line) {
            out.push_str(&format!("  -> {c}  (L{l})\n"));
            any_call = true;
        }
    }
    if !any_call {
        out.push_str("  (no outgoing calls indexed)\n");
    }
    Ok(Some(out))
}

/// Read-smart: ranked FTS5 match against `symbols_fts`. Returns top hits as
/// `file:Lstart-Lend\n<signature>` blocks. Caller fetches bodies via native
/// `Read` with the line ranges if they want more.
///
/// Returns Ok(None) if no index exists.
pub fn try_read_smart(question: &str, root: &Path, limit: usize) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;
    // Sanitize: split into alphanumeric tokens, OR-join with FTS5 syntax.
    // (Raw user input into MATCH would let users break the query.)
    let tokens: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_lowercase())
        .collect();
    if tokens.is_empty() {
        return Ok(Some(
            "read_smart: question contained no usable tokens\n".to_string(),
        ));
    }
    let fts_query = tokens.join(" OR ");

    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, coalesce(s.signature,''), f.path, s.line_start, s.line_end,
                bm25(symbols_fts) AS rank
         FROM symbols_fts
         JOIN symbols s ON s.id = symbols_fts.rowid
         JOIN files   f ON f.id = s.file_id
         WHERE symbols_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;
    let rows: Vec<(String, String, String, String, i64, i64, f64)> = stmt
        .query_map(params![fts_query, limit as i64], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if toon::enabled() {
        let label = format!("read_smart {:?} -- top {} hits", question, rows.len());
        if rows.is_empty() {
            return Ok(Some(format!("{label}\n(no matches)\n")));
        }
        let toon_rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(kind, sname, sig, path, a, b, _)| {
                vec![kind, sname, path, a.to_string(), b.to_string(), sig]
            })
            .collect();
        return Ok(Some(toon::table(
            &label,
            &["kind", "name", "path", "start", "end", "sig"],
            &toon_rows,
        )));
    }
    let mut out = String::new();
    out.push_str(&format!(
        "read_smart {:?} -> top {} of FTS matches\n",
        question,
        rows.len()
    ));
    if rows.is_empty() {
        out.push_str("(no matches — try different keywords or tkr_index_build)\n");
    } else {
        for (kind, sname, sig, path, a, b, _) in rows {
            out.push_str(&format!("\n{kind} {sname}  {path}:{a}-{b}\n"));
            if !sig.is_empty() {
                out.push_str(&format!("  {sig}\n"));
            }
        }
        out.push_str("\nhint: use native Read with offset/limit on these line ranges for bodies.\n");
    }
    Ok(Some(out))
}

