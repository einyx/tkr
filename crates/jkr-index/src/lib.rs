//! jkr-index — persistent, content-addressed code index.
//!
//! One SQLite DB per repo, living under `.jkr/index.sqlite`. Indexing is
//! incremental: a file is re-parsed only when its sha256 changes.

pub mod bundle;
mod indexer;
mod lang;
mod schema;
pub mod watch;

pub use bundle::{fetch, publish, BlobStore, Cid, IndexManifest, LocalBlobStore, MemBlobStore};
pub use indexer::IndexStats;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub use schema::SCHEMA_VERSION;

pub struct IndexDb {
    pub(crate) conn: Connection,
    #[allow(dead_code)]
    repo_root: PathBuf,
}

impl IndexDb {
    /// Open (or create) the index DB under `<repo_root>/.jkr/index.sqlite`.
    pub fn open(repo_root: impl AsRef<Path>) -> Result<Self> {
        let repo_root = repo_root.as_ref().to_path_buf();
        let dir = repo_root.join(".jkr");
        std::fs::create_dir_all(&dir).context("create .jkr/")?;
        let conn = Connection::open(dir.join("index.sqlite"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(schema::SCHEMA_SQL)?;
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', ?1)",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(Self { conn, repo_root })
    }

    /// True if `path` is already indexed at the given content hash.
    pub fn is_fresh(&self, path: &str, content_hash: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT content_hash FROM files WHERE path = ?1")?;
        let existing: Option<String> = stmt
            .query_row(params![path], |r| r.get(0))
            .ok();
        Ok(existing.as_deref() == Some(content_hash))
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_creates_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let db = IndexDb::open(tmp.path()).unwrap();
        let v: String = db
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }

    #[test]
    fn index_rust_file_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("foo.rs");
        std::fs::write(
            &src,
            "fn alpha() {}\nstruct Beta;\nfn gamma(x: i32) -> i32 { x }\n",
        )
        .unwrap();
        let mut db = IndexDb::open(tmp.path()).unwrap();
        let stats = db.index_file(&src).unwrap();
        assert_eq!(stats.symbols, 3);
        // Re-indexing the same content is a no-op.
        let stats2 = db.index_file(&src).unwrap();
        assert!(stats2.skipped_unchanged);

        // FTS search picks up "gamma".
        let n: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM symbols_fts WHERE symbols_fts MATCH 'gamma'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn indexes_new_languages_smoke() {
        let cases: &[(&str, &str, &str)] = &[
            ("a.java", "class Foo { void bar() {} }", "bar"),
            ("a.c", "int main(void) { return 0; }", "main"),
            ("a.cpp", "namespace ns { class Foo {}; }", "Foo"),
            ("a.rb", "class Foo\n  def bar; end\nend\n", "bar"),
        ];
        for (fname, src, expected) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let p = tmp.path().join(fname);
            std::fs::write(&p, src).unwrap();
            let mut db = IndexDb::open(tmp.path()).unwrap();
            let stats = db.index_file(&p).unwrap();
            assert!(stats.symbols >= 1, "{fname}: no symbols extracted");
            let hit: i64 = db
                .conn
                .query_row(
                    "SELECT count(*) FROM symbols WHERE name=?1",
                    rusqlite::params![expected],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hit, 1, "{fname}: expected to find symbol {expected}");
        }
    }

    #[test]
    fn indexes_calls_and_resolves_caller() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("foo.rs");
        std::fs::write(
            &src,
            "fn helper() {}\nfn caller() { helper(); helper(); }\n",
        )
        .unwrap();
        let mut db = IndexDb::open(tmp.path()).unwrap();
        db.index_file(&src).unwrap();
        let n: i64 = db
            .conn
            .query_row(
                "SELECT count(*) FROM refs r
                 JOIN symbols s ON s.id = r.from_symbol_id
                 WHERE r.to_name='helper' AND s.name='caller'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn freshness_check() {
        let tmp = tempfile::tempdir().unwrap();
        let db = IndexDb::open(tmp.path()).unwrap();
        assert!(!db.is_fresh("src/foo.rs", "abc").unwrap());
        db.conn
            .execute(
                "INSERT INTO files(path, lang, content_hash, mtime_ns, indexed_at)
                 VALUES('src/foo.rs', 'rust', 'abc', 0, 0)",
                [],
            )
            .unwrap();
        assert!(db.is_fresh("src/foo.rs", "abc").unwrap());
        assert!(!db.is_fresh("src/foo.rs", "xyz").unwrap());
    }
}
