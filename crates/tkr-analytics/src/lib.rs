pub mod v2;
pub use v2::{
    AnalyticsPluginV2,
    nearest_noise_embeddings_via_host,
    nearest_to_signature_via_host,
    noise_signature_id_via_host,
    noise_signatures_without_embeddings_via_host,
    record_command_stat_via_host,
    record_noise_signature_via_host,
    top_noise_signatures_via_host,
    total_savings_via_host,
    upsert_noise_embedding_via_host,
};

use anyhow::Result;
use rusqlite::{params, Connection};

// Legacy AnalyticsPlugin (implements LegacyPlugin) has been removed in favour
// of AnalyticsPluginV2 (implements tkr_api::plugin::Plugin, vault-backed).
// AnalyticsStore is kept for the `tkr gain` / `tkr suggest` read path.

pub struct AnalyticsStore {
    conn: Connection,
}

pub struct SavingsRow {
    pub command: String,
    pub tokens_in: u64,
    pub tokens_saved: u64,
    pub runs: u64,
}

pub struct NoiseRow {
    pub command: String,
    pub signature: String,
    pub sample: String,
    pub occurrences: u64,
    pub total_chars: u64,
}

impl AnalyticsStore {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS command_stats (
                id          INTEGER PRIMARY KEY,
                command     TEXT NOT NULL,
                chars_in    INTEGER NOT NULL DEFAULT 0,
                chars_saved INTEGER NOT NULL DEFAULT 0,
                runs        INTEGER NOT NULL DEFAULT 0,
                UNIQUE(command)
            );
            CREATE TABLE IF NOT EXISTS noise_signatures (
                id          INTEGER PRIMARY KEY,
                command     TEXT NOT NULL,
                signature   TEXT NOT NULL,
                sample      TEXT NOT NULL,
                occurrences INTEGER NOT NULL DEFAULT 0,
                total_chars INTEGER NOT NULL DEFAULT 0,
                last_seen   INTEGER NOT NULL,
                UNIQUE(command, signature)
            );
            CREATE INDEX IF NOT EXISTS idx_noise_command ON noise_signatures(command);",
        )?;
        Ok(Self { conn })
    }

    pub fn record_signature(
        &self,
        command: &str,
        signature: &str,
        sample: &str,
        occurrences: u64,
        chars: u64,
    ) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO noise_signatures (command, signature, sample, occurrences, total_chars, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(command, signature) DO UPDATE SET
               occurrences = occurrences + excluded.occurrences,
               total_chars = total_chars + excluded.total_chars,
               last_seen   = excluded.last_seen",
            params![command, signature, sample, occurrences as i64, chars as i64, now],
        )?;
        Ok(())
    }

    pub fn top_noise_signatures(&self, min_occurrences: u64, min_chars: u64) -> Result<Vec<NoiseRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT command, signature, sample, occurrences, total_chars
             FROM noise_signatures
             WHERE occurrences >= ?1 AND total_chars >= ?2
             ORDER BY total_chars DESC",
        )?;
        let rows = stmt.query_map(params![min_occurrences as i64, min_chars as i64], |row| {
            Ok(NoiseRow {
                command: row.get(0)?,
                signature: row.get(1)?,
                sample: row.get(2)?,
                occurrences: row.get::<_, i64>(3)? as u64,
                total_chars: row.get::<_, i64>(4)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }

    pub fn record(&self, cmd: &str, subcmd: &str, chars_in: u64, chars_saved: u64) -> Result<()> {
        let key = format!("{cmd} {subcmd}").trim().to_string();
        self.conn.execute(
            "INSERT INTO command_stats (command, chars_in, chars_saved, runs)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(command) DO UPDATE SET
               chars_in    = chars_in    + excluded.chars_in,
               chars_saved = chars_saved + excluded.chars_saved,
               runs        = runs        + 1",
            params![key, chars_in as i64, chars_saved as i64],
        )?;
        Ok(())
    }

    pub fn total_savings(&self) -> Result<Vec<SavingsRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT command, chars_in, chars_saved, runs FROM command_stats ORDER BY chars_saved DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SavingsRow {
                command: row.get(0)?,
                tokens_in: (row.get::<_, i64>(1)? / 4) as u64,
                tokens_saved: (row.get::<_, i64>(2)? / 4) as u64,
                runs: row.get::<_, i64>(3)? as u64,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_query_savings() {
        let store = AnalyticsStore::open(":memory:").unwrap();
        store.record("git", "status", 400, 300).unwrap();
        store.record("git", "status", 200, 150).unwrap();
        let rows = store.total_savings().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "git status");
        assert_eq!(rows[0].tokens_saved, 112);
    }

    #[test]
    fn record_and_query_signatures() {
        let store = AnalyticsStore::open(":memory:").unwrap();
        store.record_signature("git status", "[T] hello", "[2026-01-01T00:00:00] hello", 3, 90).unwrap();
        store.record_signature("git status", "[T] hello", "[2026-01-01T00:00:00] hello", 4, 120).unwrap();
        store.record_signature("git status", "rare", "rare", 1, 4).unwrap();
        let rows = store.top_noise_signatures(5, 100).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].signature, "[T] hello");
        assert_eq!(rows[0].occurrences, 7);
        assert_eq!(rows[0].total_chars, 210);
    }

    #[test]
    fn empty_db_returns_no_rows() {
        let store = AnalyticsStore::open(":memory:").unwrap();
        let rows = store.total_savings().unwrap();
        assert!(rows.is_empty());
    }
}
