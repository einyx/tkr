//! Index schema. Versioned — bump SCHEMA_VERSION and add a migration on any change.
//!
//! Design notes:
//! - `files.content_hash` is sha256 of file bytes. Re-index only when it changes.
//! - `symbols.parent_id` lets us model nested scopes (methods inside classes,
//!   closures inside functions) without a separate table.
//! - `refs.to_name` is the unresolved callee name as written at the call site.
//!   We deliberately do NOT resolve to a `to_symbol_id` here — cross-file
//!   resolution is language-specific and belongs in a higher layer that can
//!   query this table. Storing the raw name keeps indexing O(file).
//! - `symbols_fts` is an FTS5 contentless shadow table over `name + signature`
//!   so `grep_summary` becomes a ranked query instead of a filesystem scan.

pub const SCHEMA_VERSION: i32 = 1;

pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    lang         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    mtime_ns     INTEGER NOT NULL,
    indexed_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id         INTEGER PRIMARY KEY,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    parent_id  INTEGER REFERENCES symbols(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,         -- function, method, class, struct, enum, const, ...
    signature  TEXT,                  -- one-liner: fn foo(a: i32) -> Result<()>
    line_start INTEGER NOT NULL,
    line_end   INTEGER NOT NULL,
    docstring  TEXT
);
CREATE INDEX IF NOT EXISTS idx_symbols_file    ON symbols(file_id);
CREATE INDEX IF NOT EXISTS idx_symbols_name    ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_parent  ON symbols(parent_id);

CREATE TABLE IF NOT EXISTS refs (
    id             INTEGER PRIMARY KEY,
    from_symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    to_name        TEXT NOT NULL,     -- unresolved callee identifier
    line           INTEGER NOT NULL,
    kind           TEXT NOT NULL      -- call, import, type_ref
);
CREATE INDEX IF NOT EXISTS idx_refs_from ON refs(from_symbol_id);
CREATE INDEX IF NOT EXISTS idx_refs_to   ON refs(to_name);

CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    name, signature, docstring,
    content='symbols', content_rowid='id',
    tokenize='unicode61'
);

-- Keep FTS in sync via triggers.
CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
    INSERT INTO symbols_fts(rowid, name, signature, docstring)
    VALUES (new.id, new.name, coalesce(new.signature,''), coalesce(new.docstring,''));
END;
CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
    INSERT INTO symbols_fts(symbols_fts, rowid, name, signature, docstring)
    VALUES('delete', old.id, old.name, coalesce(old.signature,''), coalesce(old.docstring,''));
END;
"#;
