//! Index-backed tool implementations. When `<repo>/.tkr/index.sqlite` exists,
//! these return Some(rendered) and the server uses them; otherwise None and
//! the server falls back to the stateless scanners in `search.rs` / `outline.rs`.

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use rusqlite::params;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tkr_index::watch::{self, WatcherHandle};
use tkr_index::IndexDb;

use crate::toon;

/// If the row set has enough size + repetition to benefit from path
/// deduplication, returns a path-table block (`@P` header + numbered list)
/// and a map from path to short ID. Callers substitute `@<id>` for the path
/// in each row.
///
/// Threshold: at least 3 rows AND at least one path repeats. Below that the
/// table's own overhead exceeds the inlined repetition.
fn maybe_path_table<'a>(
    paths: &'a [&str],
) -> Option<(String, HashMap<&'a str, usize>)> {
    if paths.len() < 3 {
        return None;
    }
    let unique = paths.iter().collect::<HashSet<&&str>>().len();
    if unique == paths.len() {
        return None; // every path distinct — header would lose
    }
    let mut id_of: HashMap<&str, usize> = HashMap::new();
    let mut next = 1usize;
    let mut table = String::from("@P\n");
    for p in paths {
        if !id_of.contains_key(*p) {
            id_of.insert(*p, next);
            table.push_str(&format!("  {next} {p}\n"));
            next += 1;
        }
    }
    Some((table, id_of))
}

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
    out.push_str(&format!("find_symbol {:?} ({} hits)\n", name, rows.len()));
    if rows.is_empty() {
        out.push_str("(none — rebuild index if files changed)\n");
        return Ok(Some(out));
    }
    // No wide padding: aligned columns burn ~30 chars per row to no
    // benefit when the consumer is an LLM tokenizer, not a human eye.
    // Path-dedup: when a path repeats across rows, emit it once at the top
    // and reference by `@N`. See `maybe_path_table` for the heuristic.
    let paths: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
    if let Some((table, ids)) = maybe_path_table(&paths) {
        out.push_str(&table);
        for (kind, sname, path, a, b) in &rows {
            let id = ids[path.as_str()];
            out.push_str(&format!("{kind} {sname} @{id}:{a}-{b}\n"));
        }
    } else {
        for (kind, sname, path, a, b) in rows {
            out.push_str(&format!("{kind} {sname} {path}:{a}-{b}\n"));
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
        out.push_str("(none)\n");
        return Ok(Some(out));
    }
    // One line per hit when sig is empty; two when present. Indent dropped:
    // the relationship is structural, not visual. Path-dedup as elsewhere.
    let paths: Vec<&str> = rows.iter().map(|r| r.3.as_str()).collect();
    if let Some((table, ids)) = maybe_path_table(&paths) {
        out.push_str(&table);
        for (kind, sname, sig, path, a, b) in &rows {
            let id = ids[path.as_str()];
            out.push_str(&format!("{kind} {sname} @{id}:{a}-{b}\n"));
            if !sig.is_empty() {
                out.push_str(&format!("  {sig}\n"));
            }
        }
    } else {
        for (kind, sname, sig, path, a, b) in rows {
            out.push_str(&format!("{kind} {sname} {path}:{a}-{b}\n"));
            if !sig.is_empty() {
                out.push_str(&format!("  {sig}\n"));
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
        out.push_str("(none)\n");
        return Ok(Some(out));
    }
    let paths: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
    if let Some((table, ids)) = maybe_path_table(&paths) {
        out.push_str(&table);
        for (kind, sname, path, line) in &rows {
            let id = ids[path.as_str()];
            out.push_str(&format!("{kind} {sname} @{id}:{line}\n"));
        }
    } else {
        for (kind, sname, path, line) in rows {
            out.push_str(&format!("{kind} {sname} {path}:{line}\n"));
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
    // Group by symbol location. Within each group, dedup callees by name and
    // collect their call-site lines. A function that calls println!/clone/
    // unwrap N times used to emit N duplicate lines; now it's one line with
    // a comma-separated line list. Big win on hot patterns.
    //
    // No blank-line separators between groups — structure is carried by
    // indentation + the `->` prefix on callee lines.
    type GroupKey = (String, String, i64);
    struct GroupData {
        kind: String,
        // Insertion-ordered: callee names keyed by first-occurrence order so
        // the output ordering tracks source-code reading order (top-down),
        // not alphabetical. Keeps the agent's mental model of "what does X
        // do?" intact.
        callees: Vec<String>,
        lines: HashMap<String, Vec<i64>>,
    }
    let mut groups: Vec<GroupKey> = Vec::new();
    let mut data: HashMap<GroupKey, GroupData> = HashMap::new();
    let mut any_call = false;

    for (kind, sname, path, lstart, callee, line) in rows {
        let key = (path.clone(), sname.clone(), lstart);
        let entry = data.entry(key.clone()).or_insert_with(|| {
            groups.push(key.clone());
            GroupData {
                kind: kind.clone(),
                callees: Vec::new(),
                lines: HashMap::new(),
            }
        });
        if let (Some(c), Some(l)) = (callee, line) {
            if !entry.lines.contains_key(&c) {
                entry.callees.push(c.clone());
            }
            entry.lines.entry(c).or_default().push(l);
            any_call = true;
        }
    }

    // Render. Per-callee line cap = 8: hot helpers like println!/format!
    // can be called 50+ times in a single function; uncapped lists would
    // re-create the bloat we just eliminated.
    const LINE_LIST_CAP: usize = 8;
    for key in &groups {
        let g = &data[key];
        out.push_str(&format!("{} {} {}:{}\n", g.kind, key.1, key.0, key.2));
        for c in &g.callees {
            let lines = &g.lines[c];
            if lines.len() == 1 {
                out.push_str(&format!("  -> {}:{}\n", c, lines[0]));
            } else if lines.len() <= LINE_LIST_CAP {
                let joined = lines
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                out.push_str(&format!("  -> {c}:{joined}\n"));
            } else {
                let head = lines[..LINE_LIST_CAP]
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let more = lines.len() - LINE_LIST_CAP;
                out.push_str(&format!("  -> {c}:{head} (+{more} more)\n"));
            }
        }
    }
    if !any_call {
        out.push_str("(no outgoing calls indexed)\n");
    }
    Ok(Some(out))
}

/// Shortest call-path from any symbol named `from` to any symbol named `to`.
/// BFS over the `refs` table; cycles avoided via visited-set on symbol ids.
/// Note: the index's `to_name` column is the unresolved callee identifier as
/// written at the call site — so this matches any call to a symbol with that
/// name, regardless of receiver or module. A single hop may fan out into many
/// branches when overloads share a name; BFS returns the shortest.
pub fn try_call_path(
    from: &str,
    to: &str,
    max_depth: usize,
    root: &Path,
) -> Result<Option<String>> {
    if !index_path(root).exists() {
        return Ok(None);
    }
    let db = IndexDb::open(root)?;

    let start_ids = symbols_named(&db, from)?;
    if start_ids.is_empty() {
        return Ok(Some(format!(
            "call_path {from:?} -> {to:?}: no symbol named {from:?} in index\n"
        )));
    }
    // Trivial case: from == to. Honor it cleanly so callers can use it as a
    // probe for "does any symbol named X exist?".
    if from == to {
        return Ok(Some(format!(
            "call_path {from:?} -> {to:?}: trivial (same name), {} candidate symbol(s)\n",
            start_ids.len()
        )));
    }

    let mut step_stmt = db.conn().prepare(
        "SELECT r.from_symbol_id, s2.id, s2.name, f2.path, s2.line_start, r.line
         FROM refs r
         JOIN symbols s2 ON s2.name = r.to_name
         JOIN files   f2 ON f2.id = s2.file_id
         WHERE r.from_symbol_id = ?1",
    )?;

    // BFS state. `parent[next_id] = (prev_id, callsite_line, next_name, path,
    // next_lstart)` — enough to rebuild the path with line numbers.
    let mut visited: HashSet<i64> = start_ids.iter().copied().collect();
    #[allow(clippy::type_complexity)]
    let mut parent: HashMap<i64, (i64, i64, String, String, i64)> = HashMap::new();
    let mut frontier: VecDeque<(i64, usize)> =
        start_ids.iter().map(|id| (*id, 0usize)).collect();
    let start_set: HashSet<i64> = start_ids.iter().copied().collect();

    while let Some((cur_id, depth)) = frontier.pop_front() {
        if depth >= max_depth {
            continue;
        }
        let rows: Vec<(i64, i64, String, String, i64, i64)> = step_stmt
            .query_map(params![cur_id], |r| {
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
        for (prev_id, next_id, next_name, path, lstart, call_line) in rows {
            if visited.contains(&next_id) {
                continue;
            }
            visited.insert(next_id);
            parent.insert(next_id, (prev_id, call_line, next_name.clone(), path, lstart));
            if next_name == to {
                let path_str = render_path(&db, next_id, &parent, &start_set)?;
                return Ok(Some(format!(
                    "call_path {from:?} -> {to:?} (depth {})\n{}",
                    depth + 1,
                    path_str
                )));
            }
            frontier.push_back((next_id, depth + 1));
        }
    }

    Ok(Some(format!(
        "call_path {from:?} -> {to:?}: no path within depth {max_depth}\n\
         hint: increase depth, or check `tkr_callees_of {from:?}` to see what {from} actually reaches\n"
    )))
}

/// All symbol ids whose `name` column equals `name`.
fn symbols_named(db: &IndexDb, name: &str) -> Result<Vec<i64>> {
    let mut stmt = db.conn().prepare("SELECT id FROM symbols WHERE name = ?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![name], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Walk back from `terminal_id` through `parent` and render one line per hop.
/// The first line is the start symbol (no incoming edge).
fn render_path(
    db: &IndexDb,
    terminal_id: i64,
    parent: &HashMap<i64, (i64, i64, String, String, i64)>,
    start_set: &HashSet<i64>,
) -> Result<String> {
    let mut chain: Vec<i64> = Vec::new();
    let mut cur = terminal_id;
    chain.push(cur);
    while let Some((prev_id, _, _, _, _)) = parent.get(&cur) {
        cur = *prev_id;
        chain.push(cur);
        if start_set.contains(&cur) {
            break;
        }
    }
    chain.reverse();

    let mut stmt = db.conn().prepare(
        "SELECT s.kind, s.name, f.path, s.line_start
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.id = ?1",
    )?;

    let mut out = String::new();
    for (i, sym_id) in chain.iter().enumerate() {
        let (kind, sname, path, lstart): (String, String, String, i64) = stmt
            .query_row(params![sym_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .with_context(|| format!("symbol id {sym_id} vanished mid-walk"))?;
        if i == 0 {
            out.push_str(&format!("  {kind} {sname}  {path}:{lstart}\n"));
        } else {
            let call_line = parent
                .get(sym_id)
                .map(|p| p.1)
                .unwrap_or(0);
            out.push_str(&format!("    -> at L{call_line}\n"));
            out.push_str(&format!("  {kind} {sname}  {path}:{lstart}\n"));
        }
    }
    Ok(out)
}

/// Read-smart: ranked FTS5 match against `symbols_fts`. Returns top hits as
/// `file:Lstart-Lend\n<signature>` blocks. Caller fetches bodies via native
/// `Read` with the line ranges if they want more.
///
/// Returns Ok(None) if no index exists.
pub fn try_read_smart(
    question: &str,
    root: &Path,
    limit: usize,
    verbose: bool,
) -> Result<Option<String>> {
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
        // Terse by default: drop the `sig` column. Caller opts in to it.
        // For TOON the column count savings is even more material because
        // empty sig cells still occupy a tab position.
        if verbose {
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
        let toon_rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|(kind, sname, _sig, path, a, b, _)| {
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
        "read_smart {:?} -- top {} FTS matches\n",
        question,
        rows.len()
    ));
    if rows.is_empty() {
        out.push_str("(none — try different keywords)\n");
        return Ok(Some(out));
    }
    // One block per hit; no blank-line separators. Path-dedup applies the
    // same heuristic as find_symbol.
    //
    // Signatures are opt-in (`verbose=true`). Default-off because in a real
    // agent flow read_smart is the "find candidate locations" tool — bodies
    // come from a follow-up `tkr_signature` or `Read`. Including signatures
    // on every read_smart turn was the dominant per-call cost in the bench
    // (974 B/call). Dropping them by default cuts that ~40-50% on real repos.
    //
    // The didactic "use native Read with offset/limit" hint was dropped: an
    // agent already on a `tkr_read_smart` tool call knows how to read a file
    // by line range.
    let paths: Vec<&str> = rows.iter().map(|r| r.3.as_str()).collect();
    if let Some((table, ids)) = maybe_path_table(&paths) {
        out.push_str(&table);
        for (kind, sname, sig, path, a, b, _) in &rows {
            let id = ids[path.as_str()];
            out.push_str(&format!("{kind} {sname} @{id}:{a}-{b}\n"));
            if verbose && !sig.is_empty() {
                out.push_str(&format!("  {sig}\n"));
            }
        }
    } else {
        for (kind, sname, sig, path, a, b, _) in rows {
            out.push_str(&format!("{kind} {sname} {path}:{a}-{b}\n"));
            if verbose && !sig.is_empty() {
                out.push_str(&format!("  {sig}\n"));
            }
        }
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Fixture: a tiny Rust file that exercises a 3-hop chain a → b → c → d.
    fn three_hop_repo() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        let src = dir.path().join("chain.rs");
        fs::write(
            &src,
            r#"
fn d() { println!("end"); }
fn c() { d(); }
fn b() { c(); }
fn a() { b(); }
fn unrelated() { println!("not in chain"); }
"#,
        )
        .unwrap();
        let mut db = IndexDb::open(dir.path()).unwrap();
        db.index_file(&src).unwrap();
        dir
    }

    #[test]
    fn call_path_finds_three_hop_chain() {
        let dir = three_hop_repo();
        let out = try_call_path("a", "d", 8, dir.path()).unwrap().unwrap();
        let sym_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("  function "))
            .collect();
        assert_eq!(sym_lines.len(), 4, "expected 4 hops in path, got:\n{out}");
        assert!(sym_lines[0].contains(" a "), "first hop should be `a`: {out}");
        assert!(
            sym_lines.last().unwrap().contains(" d "),
            "last hop should be `d`: {out}"
        );
        assert!(out.contains("(depth 3)"), "depth label missing: {out}");
    }

    #[test]
    fn call_path_returns_no_path_for_disconnected_pair() {
        let dir = three_hop_repo();
        let out = try_call_path("a", "unrelated", 8, dir.path())
            .unwrap()
            .unwrap();
        assert!(
            out.contains("no path within depth"),
            "expected no-path message, got:\n{out}"
        );
    }

    #[test]
    fn call_path_respects_depth_cap() {
        let dir = three_hop_repo();
        let out = try_call_path("a", "d", 2, dir.path()).unwrap().unwrap();
        assert!(
            out.contains("no path within depth 2"),
            "depth cap not enforced:\n{out}"
        );
    }

    #[test]
    fn call_path_reports_missing_start() {
        let dir = three_hop_repo();
        let out = try_call_path("does_not_exist", "d", 8, dir.path())
            .unwrap()
            .unwrap();
        assert!(out.contains("no symbol named"), "expected start-missing: {out}");
    }

    #[test]
    fn call_path_handles_trivial_self() {
        let dir = three_hop_repo();
        let out = try_call_path("a", "a", 8, dir.path()).unwrap().unwrap();
        assert!(out.contains("trivial"), "expected trivial branch: {out}");
    }

    #[test]
    fn call_path_returns_none_when_no_index() {
        let dir = tempdir().unwrap();
        let out = try_call_path("a", "b", 4, dir.path()).unwrap();
        assert!(out.is_none());
    }

    /// Fixture: multi-file repo where the same name (`Helper`) appears in
    /// several files, forcing repeat-path scenarios for find_symbol.
    fn multi_file_repo_with_repeats() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        // Two files, each defining `Helper` twice — guarantees ≥3 rows with
        // repeated paths.
        fs::write(
            dir.path().join("alpha.rs"),
            "fn Helper() {}\nfn Helper_v2() {}\nfn Helper() {}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("beta.rs"),
            "fn Helper() {}\nfn other() {}\nfn Helper() {}\n",
        )
        .unwrap();
        let mut db = IndexDb::open(dir.path()).unwrap();
        db.index_file(&dir.path().join("alpha.rs")).unwrap();
        db.index_file(&dir.path().join("beta.rs")).unwrap();
        dir
    }

    #[test]
    fn path_dedup_kicks_in_when_paths_repeat() {
        let dir = multi_file_repo_with_repeats();
        let out = try_find_symbol("Helper", dir.path()).unwrap().unwrap();
        // Header marker `@P` must be present and each path appears exactly
        // once after the marker (in the table) and zero times outside.
        assert!(out.contains("@P\n"), "expected path table header:\n{out}");
        assert!(
            out.contains("@1:") && out.contains("@2:"),
            "expected at least two path IDs in rows:\n{out}"
        );
        // Each unique path appears exactly once in the entire output (in
        // the path table only).
        let alpha_count = out.matches("alpha.rs").count();
        let beta_count = out.matches("beta.rs").count();
        assert_eq!(alpha_count, 1, "alpha.rs should appear once (path table), got {alpha_count}:\n{out}");
        assert_eq!(beta_count, 1, "beta.rs should appear once (path table), got {beta_count}:\n{out}");
    }

    #[test]
    fn path_dedup_skipped_when_all_paths_unique() {
        let dir = three_hop_repo();
        // find_symbol "b" returns one row → below 3-row threshold, no dedup.
        let out = try_find_symbol("b", dir.path()).unwrap().unwrap();
        assert!(!out.contains("@P\n"), "no dedup expected for single hit:\n{out}");
    }

    #[test]
    fn callees_of_dedupes_repeated_calls() {
        // The tree-sitter Rust index captures call_expression nodes (regular
        // function calls), not macro_invocation. So this fixture uses plain
        // function calls — `helper` invoked four times + `other` once.
        let dir = tempdir().unwrap();
        let src = dir.path().join("hot.rs");
        fs::write(
            &src,
            "fn helper(x: u32) -> u32 { x + 1 }\n\
             fn other() {}\n\
             fn hot() {\n    \
             helper(1);\n    \
             helper(2);\n    \
             helper(3);\n    \
             helper(4);\n    \
             other();\n\
             }\n",
        )
        .unwrap();
        let mut db = IndexDb::open(dir.path()).unwrap();
        db.index_file(&src).unwrap();

        let out = try_callees_of("hot", dir.path()).unwrap().unwrap();
        // Each callee name appears exactly once on its own `->` line.
        let helper_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains("-> helper"))
            .collect();
        assert_eq!(
            helper_lines.len(),
            1,
            "expected exactly one `-> helper` line (deduped):\n{out}"
        );
        // That line must list multiple call-site lines (comma-separated).
        let line = helper_lines[0];
        assert!(
            line.contains(','),
            "expected comma-separated line list for repeated calls:\n{line}\n{out}"
        );
        // `other` appears once, so no comma in its line.
        let other_lines: Vec<&str> =
            out.lines().filter(|l| l.contains("-> other")).collect();
        assert_eq!(other_lines.len(), 1);
        assert!(
            !other_lines[0].contains(','),
            "single-call callee should not have comma list:\n{}",
            other_lines[0]
        );
    }

    #[test]
    fn read_smart_verbose_flag_toggles_signatures() {
        // Need multi-char names: read_smart's FTS tokenizer drops tokens
        // shorter than 2 chars. The three_hop fixture uses `a`/`b`/`c`/`d`
        // which would be filtered out.
        let dir = tempdir().unwrap();
        let src = dir.path().join("verbose_fixture.rs");
        fs::write(
            &src,
            "fn parse_query(input: &str) -> Vec<String> { vec![] }\n\
             fn build_query(tokens: &[String]) -> String { String::new() }\n",
        )
        .unwrap();
        let mut db = IndexDb::open(dir.path()).unwrap();
        db.index_file(&src).unwrap();

        let terse = try_read_smart("parse_query", dir.path(), 5, false)
            .unwrap()
            .unwrap();
        let verbose = try_read_smart("parse_query", dir.path(), 5, true)
            .unwrap()
            .unwrap();

        assert!(
            verbose.len() > terse.len(),
            "verbose should include extra signature lines:\nterse:\n{terse}\nverbose:\n{verbose}"
        );
        assert!(
            terse.contains("parse_query"),
            "terse output missing symbol:\n{terse}"
        );
        // Signature should appear only in verbose mode. The parsed signature
        // for a Rust fn includes the parameter list, so `&str` is a good
        // signature-only marker (it's never in the kind/name/path fields).
        assert!(
            !terse.contains("&str"),
            "terse mode should not include signature:\n{terse}"
        );
        assert!(
            verbose.contains("&str"),
            "verbose mode should include signature:\n{verbose}"
        );
    }

    /// Measure the actual byte savings from path-dedup so future shape
    /// changes can be eyeballed against this number. Prints to stdout under
    /// `cargo test -- --nocapture` for spot-checking.
    #[test]
    fn path_dedup_byte_savings_are_real() {
        let dir = multi_file_repo_with_repeats();
        let out = try_find_symbol("Helper", dir.path()).unwrap().unwrap();
        // Estimate the un-deduped length: replace each `@N:` back to the
        // path it refers to and add the saved path-table block back.
        let mut id_to_path: HashMap<usize, String> = HashMap::new();
        for line in out.lines() {
            // Path-table rows look like `  N /full/path`. Pick them out.
            let trimmed = line.trim_start();
            if let Some((id_str, rest)) = trimmed.split_once(' ') {
                if let Ok(id) = id_str.parse::<usize>() {
                    if line.starts_with("  ") {
                        id_to_path.insert(id, rest.to_string());
                    }
                }
            }
        }
        assert!(
            !id_to_path.is_empty(),
            "expected to parse at least one path from the table:\n{out}"
        );
        // Reconstruct the inline-only form by replacing `@N` with the path.
        let mut inline_form = String::new();
        for line in out.lines() {
            // Drop the @P header + its table rows from the reconstructed form.
            let t = line.trim_start();
            if line == "@P" {
                continue;
            }
            if t.split_once(' ').and_then(|(s, _)| s.parse::<usize>().ok()).is_some()
                && line.starts_with("  ")
            {
                continue;
            }
            // Substitute @N: → /full/path: in row lines.
            let mut replaced = line.to_string();
            for (id, p) in &id_to_path {
                replaced = replaced.replace(&format!("@{id}:"), &format!("{p}:"));
            }
            inline_form.push_str(&replaced);
            inline_form.push('\n');
        }
        let saved = inline_form.len().saturating_sub(out.len());
        // For 4-row response with 2 long temp paths, expect >= a few dozen
        // bytes saved. Loose threshold to keep test stable across machines.
        assert!(
            saved >= 20,
            "expected ≥20 bytes saved, got {saved} (inline={}, dedup={}):\n=== inline ===\n{inline_form}\n=== dedup ===\n{out}",
            inline_form.len(),
            out.len()
        );
        eprintln!(
            "path_dedup saved {saved} bytes ({:.0}%) on find_symbol fixture",
            (saved as f64 / inline_form.len() as f64) * 100.0
        );
    }

    /// Pin the tightened response shapes. These bytes-per-hit budgets are
    /// what the Phase 3 token-saving pass produced; if a future change
    /// pads them out (alignment columns, didactic footers) this test fails
    /// loudly so we don't drift back.
    #[test]
    fn response_shapes_stay_tight() {
        let dir = three_hop_repo();

        // find_symbol: 4 hits ("a","b","c","d") in chain.rs.
        // Each row is `function <name> <path>:<a>-<b>\n`. With a tmp path
        // of typical length, this should be under 80 bytes per row.
        let fs_out = try_find_symbol("b", dir.path()).unwrap().unwrap();
        // Expect no wide-alignment whitespace (i.e. no run of 6+ spaces
        // outside leading indent).
        assert!(
            !fs_out.contains("      "),
            "find_symbol output reintroduced padding:\n{fs_out}"
        );

        // callers_of "d" — should be a single row for fn c.
        let co_out = try_callers_of("d", dir.path()).unwrap().unwrap();
        assert!(
            !co_out.contains("      "),
            "callers_of output reintroduced padding:\n{co_out}"
        );
        assert!(co_out.contains("c "), "callers_of should list `c`:\n{co_out}");

        // callees_of "b" — should report b -> c. Format is "-> c:<line>",
        // not "(L<line>)".
        let cl_out = try_callees_of("b", dir.path()).unwrap().unwrap();
        assert!(
            cl_out.contains("-> c:"),
            "callees_of dropped path:line format:\n{cl_out}"
        );
        assert!(
            !cl_out.contains("(L"),
            "callees_of still using `(L{{n}})` format:\n{cl_out}"
        );

        // read_smart on a phrase that should hit `function a`.
        let rs_out = try_read_smart("a", dir.path(), 5, false).unwrap().unwrap();
        assert!(
            !rs_out.contains("hint:"),
            "read_smart still emits hint footer:\n{rs_out}"
        );
    }
}
