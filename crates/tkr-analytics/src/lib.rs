use anyhow::Result;
use rusqlite::{params, Connection};
use tkr_api::{FilterResult, LegacyPlugin as Plugin};

pub struct AnalyticsPlugin {
    store: AnalyticsStore,
    command: String,
    args: String,
    chars_in: u64,
    chars_suppressed: u64,
}

impl AnalyticsPlugin {
    pub fn new(db_path: &str) -> Result<Self> {
        Ok(Self {
            store: AnalyticsStore::open(db_path)?,
            command: String::new(),
            args: String::new(),
            chars_in: 0,
            chars_suppressed: 0,
        })
    }
}

impl Plugin for AnalyticsPlugin {
    fn init(_config: &str) -> Box<dyn Plugin> where Self: Sized {
        let home = dirs::home_dir().unwrap_or_default();
        let db = home.join(".tkr/analytics.db");
        std::fs::create_dir_all(db.parent().unwrap()).ok();
        let p = Self::new(db.to_str().unwrap_or(":memory:"))
            .unwrap_or_else(|_| Self::new(":memory:").unwrap());
        Box::new(p)
    }

    fn filter(&mut self, line: &str, command: &str, args: &str, index: u64) -> FilterResult {
        if index == 0 { self.command = command.to_string(); self.args = args.to_string(); }
        self.chars_in += line.len() as u64;
        FilterResult::Pass
    }

    fn flush(&mut self) -> String {
        let subcmd = self.args.split_whitespace().next().unwrap_or("");
        let _ = self.store.record(&self.command, subcmd, self.chars_in, self.chars_suppressed);
        self.chars_in = 0;
        self.chars_suppressed = 0;
        String::new()
    }
}

pub struct AnalyticsStore {
    conn: Connection,
}

pub struct SavingsRow {
    pub command: String,
    pub tokens_in: u64,
    pub tokens_saved: u64,
    pub runs: u64,
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
            );",
        )?;
        Ok(Self { conn })
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
    fn empty_db_returns_no_rows() {
        let store = AnalyticsStore::open(":memory:").unwrap();
        let rows = store.total_savings().unwrap();
        assert!(rows.is_empty());
    }
}
