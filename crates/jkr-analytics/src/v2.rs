use serde_json::json;
use std::sync::{Arc, Mutex};
use tkr_api::{
    bus::{Reply, Request},
    capability,
    host::Host,
    manifest::{Manifest, SensitivityClass, StorageKind, StorageRequest},
    plugin::{CommandCtx, FilterDecision, Plugin},
    Error, Result as ApiResult,
};

// ── Schema ────────────────────────────────────────────────────────────────────

const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS command_stats (
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
    CREATE INDEX IF NOT EXISTS idx_noise_command ON noise_signatures(command);
";

/// sqlite-vec `vec0` module — installed lazily so `command_stats` / `tkr gain`
/// still work when vec0 fails to load on a host (broken extension, stale build).
const SCHEMA_VEC_EMBEDDINGS: &str = "
    CREATE VIRTUAL TABLE IF NOT EXISTS noise_embeddings USING vec0(
        signature_id INTEGER PRIMARY KEY,
        embedding FLOAT[384]
    );
";

fn ensure_vec_embeddings(host: &dyn Host) -> ApiResult<()> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    db.execute(SCHEMA_VEC_EMBEDDINGS, &[])?;
    Ok(())
}

const NOISE_UPSERT_SQL: &str = "
    INSERT INTO noise_signatures (command, signature, sample, occurrences, total_chars, last_seen)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6)
    ON CONFLICT(command, signature) DO UPDATE SET
        occurrences = occurrences + excluded.occurrences,
        total_chars = total_chars + excluded.total_chars,
        last_seen   = excluded.last_seen
";

const UPSERT_SQL: &str = "
    INSERT INTO command_stats (command, chars_in, chars_saved, runs)
    VALUES (?1, ?2, ?3, 1)
    ON CONFLICT(command) DO UPDATE SET
        chars_in    = chars_in    + excluded.chars_in,
        chars_saved = chars_saved + excluded.chars_saved,
        runs        = runs        + 1
";

const SELECT_ALL_SQL: &str =
    "SELECT command, chars_in, chars_saved, runs FROM command_stats ORDER BY chars_saved DESC";

// ── Plugin struct ─────────────────────────────────────────────────────────────

pub struct AnalyticsPluginV2 {
    host: Mutex<Option<Arc<dyn Host>>>,
    /// Command key for the current command session (e.g. "git status").
    current_key: Mutex<Option<String>>,
    /// Accumulated chars_in for the current command.
    chars_in: Mutex<u64>,
}

impl AnalyticsPluginV2 {
    pub fn new() -> Self {
        Self {
            host: Mutex::new(None),
            current_key: Mutex::new(None),
            chars_in: Mutex::new(0),
        }
    }

    /// Execute a closure with the vault sqlite handle, if the host is present.
    fn with_sqlite<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&dyn tkr_api::handles::Sqlite) -> R,
    {
        let guard = self.host.lock().unwrap();
        let host = guard.as_ref()?;
        match host.sqlite(SCHEMA_SQL, SensitivityClass::Public) {
            Ok(db) => Some(f(db)),
            Err(_) => None,
        }
    }

    /// Persist a learned noise signature into the vault sqlite. Idempotent —
    /// duplicate (command, signature) pairs are upserted with summed totals.
    /// Returns true if recorded, false if no vault is available.
    pub fn record_noise_signature(
        &self,
        command: &str,
        signature: &str,
        sample: &str,
        occurrences: u64,
        total_chars: u64,
    ) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.with_sqlite(|db| {
            let _ = db.execute(
                NOISE_UPSERT_SQL,
                &[
                    json!(command),
                    json!(signature),
                    json!(sample),
                    json!(occurrences as i64),
                    json!(total_chars as i64),
                    json!(now),
                ],
            );
        })
        .is_some()
    }
}

impl Default for AnalyticsPluginV2 {
    fn default() -> Self {
        Self::new()
    }
}

// ── Plugin impl ───────────────────────────────────────────────────────────────

impl Plugin for AnalyticsPluginV2 {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "tkr-analytics".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities_required: vec![
                capability::VAULT_READ_PUBLIC.into(),
                capability::VAULT_WRITE_PUBLIC.into(),
                capability::STDOUT_FILTER.into(),
            ],
            services_exposed: vec![tkr_api::manifest::ServiceSpec {
                method: "analytics.savings".into(),
                required_caps: vec![],
            }],
            storage_requests: vec![StorageRequest {
                kind: StorageKind::Sqlite,
                class: SensitivityClass::Public,
            }],
            ..Default::default()
        }
    }

    fn on_load(&mut self, host: Arc<dyn Host>) -> ApiResult<()> {
        // Initialise schema by calling sqlite (side effect: creates tables).
        // Non-fatal: if the host can't provide sqlite storage, analytics is a no-op.
        let _ = host.sqlite(SCHEMA_SQL, SensitivityClass::Public);

        // One-shot legacy migration: only run if the legacy DB file still exists.
        // We skip reading/writing ~/.tkr/schema.json because the mere presence
        // of ~/.tkr/analytics.db is already the guard — once it's renamed to
        // .migrated the check is O(1) (stat call, ~1μs).
        // Non-fatal: migration failure is logged and ignored.
        if legacy_db_exists() {
            let _ = migrate_legacy_db(host.as_ref());
        }

        // Stash host for later use in on_command_end / on_request.
        *self.host.lock().unwrap() = Some(host);
        Ok(())
    }

    fn on_command_begin(&mut self, ctx: &CommandCtx) -> ApiResult<()> {
        let subcmd = ctx.args.split_whitespace().next().unwrap_or("");
        let key = format!("{} {}", ctx.command, subcmd).trim().to_string();
        *self.current_key.lock().unwrap() = Some(key);
        *self.chars_in.lock().unwrap() = 0;
        Ok(())
    }

    fn on_line(&mut self, line: &str, _ctx: &CommandCtx) -> ApiResult<FilterDecision> {
        *self.chars_in.lock().unwrap() += line.len() as u64;
        Ok(FilterDecision::Pass)
    }

    fn on_command_end(&mut self, _ctx: &CommandCtx) -> ApiResult<String> {
        let key = self.current_key.lock().unwrap().clone();
        let chars = *self.chars_in.lock().unwrap();
        if let Some(cmd) = key {
            self.with_sqlite(|db| {
                db.execute(
                    UPSERT_SQL,
                    &[
                        json!(cmd),
                        json!(chars as i64),
                        json!(0_i64), // chars_saved: analytics doesn't measure savings itself
                    ],
                )
            });
        }
        *self.chars_in.lock().unwrap() = 0;
        *self.current_key.lock().unwrap() = None;
        Ok(String::new())
    }

    fn on_request(&mut self, req: Request) -> ApiResult<Reply> {
        if req.method != "analytics.savings" {
            return Err(Error::UnknownMethod(req.method));
        }
        let rows = self
            .with_sqlite(|db| db.query(SELECT_ALL_SQL, &[]))
            .unwrap_or_else(|| Ok(vec![]))
            .map_err(|e| Error::Plugin(format!("query: {e}")))?;

        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|row| {
                let command = row
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let chars_in = row.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
                let chars_saved = row.get(2).and_then(|v| v.as_i64()).unwrap_or(0);
                let runs = row.get(3).and_then(|v| v.as_i64()).unwrap_or(0);
                json!({
                    "command": command,
                    "chars_in": chars_in,
                    "chars_saved": chars_saved,
                    "runs": runs,
                })
            })
            .collect();

        Ok(Reply {
            payload: json!({ "rows": items }),
        })
    }

    fn on_shutdown(&mut self) -> ApiResult<()> {
        // No-op: we persist eagerly on every on_command_end.
        Ok(())
    }
}

// ── Free functions usable without an instance (called by orchestrator) ──────

/// Upsert an embedding for an existing `noise_signatures.id` into the
/// `noise_embeddings` vec0 table. The vector is a 384-dim float array
/// (MiniLM L6 v2 dimensionality).
pub fn upsert_noise_embedding_via_host(
    host: &dyn Host,
    signature_id: i64,
    embedding: &[f32],
) -> ApiResult<()> {
    if embedding.is_empty() {
        return Ok(());
    }
    ensure_vec_embeddings(host)?;
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    // sqlite-vec expects the embedding as a JSON array or as a raw bytes blob.
    // Use the JSON form — simpler, and serde_json gives us a clean roundtrip.
    let arr = serde_json::Value::Array(
        embedding
            .iter()
            .map(|f| serde_json::Value::from(*f as f64))
            .collect(),
    )
    .to_string();
    // INSERT OR REPLACE so re-embedding the same signature is a no-op cost.
    db.execute(
        "INSERT OR REPLACE INTO noise_embeddings (signature_id, embedding) VALUES (?1, ?2)",
        &[json!(signature_id), json!(arr)],
    )?;
    Ok(())
}

/// kNN by an existing signature row. Returns up to `k` near-neighbors of
/// the embedding stored at `signature_id`, sorted nearest first. The query
/// row itself is excluded from the results (distance 0).
pub fn nearest_to_signature_via_host(
    host: &dyn Host,
    signature_id: i64,
    k: usize,
) -> ApiResult<Vec<(i64, String, String, f32)>> {
    ensure_vec_embeddings(host)?;
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    // sqlite-vec lets us pass a subquery into MATCH so we don't have to
    // round-trip the embedding bytes through Rust.
    let rows = db.query(
        "SELECT s.id, s.command, s.signature, n.distance
         FROM noise_embeddings n
         JOIN noise_signatures s ON s.id = n.signature_id
         WHERE n.embedding MATCH (
             SELECT embedding FROM noise_embeddings WHERE signature_id = ?1
         )
           AND n.signature_id != ?1
           AND k = ?2
         ORDER BY n.distance",
        &[json!(signature_id), json!(k as i64)],
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some((
                r.first()?.as_i64()?,
                r.get(1)?.as_str()?.to_string(),
                r.get(2)?.as_str()?.to_string(),
                r.get(3)?.as_f64()? as f32,
            ))
        })
        .collect())
}

/// kNN over `noise_embeddings`. Returns up to `k` (signature_id, distance)
/// pairs, sorted nearest first. Distance is L2 by default in sqlite-vec.
pub fn nearest_noise_embeddings_via_host(
    host: &dyn Host,
    query_embedding: &[f32],
    k: usize,
) -> ApiResult<Vec<(i64, f32)>> {
    if query_embedding.is_empty() {
        return Ok(Vec::new());
    }
    ensure_vec_embeddings(host)?;
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    let arr = serde_json::Value::Array(
        query_embedding
            .iter()
            .map(|f| serde_json::Value::from(*f as f64))
            .collect(),
    )
    .to_string();
    let rows = db.query(
        "SELECT signature_id, distance
         FROM noise_embeddings
         WHERE embedding MATCH ?1 AND k = ?2
         ORDER BY distance",
        &[json!(arr), json!(k as i64)],
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r.first()?.as_i64()?, r.get(1)?.as_f64()? as f32)))
        .collect())
}

/// Look up the `noise_signatures.id` for a given (command, signature) pair.
pub fn noise_signature_id_via_host(
    host: &dyn Host,
    command: &str,
    signature: &str,
) -> ApiResult<Option<i64>> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    let rows = db.query(
        "SELECT id FROM noise_signatures WHERE command = ?1 AND signature = ?2 LIMIT 1",
        &[json!(command), json!(signature)],
    )?;
    Ok(rows.into_iter().next().and_then(|r| r.first()?.as_i64()))
}

/// Return all noise_signatures rows that have no corresponding row in the
/// noise_embeddings table — the work queue for the lazy embed-and-persist
/// step in `tkr suggest`.
pub fn noise_signatures_without_embeddings_via_host(
    host: &dyn Host,
    limit: usize,
) -> ApiResult<Vec<(i64, String)>> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    let lim = json!(limit as i64);
    let rows = if db.execute(SCHEMA_VEC_EMBEDDINGS, &[]).is_ok() {
        db.query(
            "SELECT s.id, s.sample
             FROM noise_signatures s
             WHERE NOT EXISTS (
                 SELECT 1 FROM noise_embeddings e WHERE e.signature_id = s.id
             )
             ORDER BY s.total_chars DESC
             LIMIT ?1",
            &[lim],
        )?
    } else {
        db.query(
            "SELECT id, sample FROM noise_signatures ORDER BY total_chars DESC LIMIT ?1",
            &[lim],
        )?
    };
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r.first()?.as_i64()?, r.get(1)?.as_str()?.to_string())))
        .collect())
}

/// Upsert a command-stats row into the vault sqlite using any `Host` impl.
/// Lets non-plugin code (proxy::run) record per-run savings directly without
/// going through the legacy session-socket / `tkr watch` daemon path.
pub fn record_command_stat_via_host(
    host: &dyn Host,
    command: &str,
    chars_in: u64,
    chars_saved: u64,
) -> ApiResult<()> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    db.execute(
        UPSERT_SQL,
        &[
            json!(command),
            json!(chars_in as i64),
            json!(chars_saved as i64),
        ],
    )?;
    Ok(())
}

/// Persist a noise signature into the vault sqlite using any `Host` impl.
/// Lets non-plugin code (proxy::run) write learning data directly.
pub fn record_noise_signature_via_host(
    host: &dyn Host,
    command: &str,
    signature: &str,
    sample: &str,
    occurrences: u64,
    total_chars: u64,
) -> ApiResult<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    db.execute(
        NOISE_UPSERT_SQL,
        &[
            json!(command),
            json!(signature),
            json!(sample),
            json!(occurrences as i64),
            json!(total_chars as i64),
            json!(now),
        ],
    )?;
    Ok(())
}

// ── Read-side helpers usable without a plugin instance ───────────────────────

/// Read all command_stats rows from the vault sqlite, returning them as
/// `SavingsRow` values. Characters are divided by 4 to approximate tokens
/// (same conversion as the legacy `AnalyticsStore::total_savings`).
///
/// Returns an empty `Vec` if no rows exist or the host cannot open sqlite.
pub fn total_savings_via_host(host: &dyn Host) -> ApiResult<Vec<crate::SavingsRow>> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    let rows = db.query(SELECT_ALL_SQL, &[])?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let command = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let chars_in = row.get(1).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;
            let chars_saved = row.get(2).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;
            let runs = row.get(3).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;
            crate::SavingsRow {
                command,
                tokens_in: chars_in / 4,
                tokens_saved: chars_saved / 4,
                runs,
            }
        })
        .collect())
}

/// Read the top noise signatures from the vault sqlite. Only rows with at least
/// `min_occurrences` occurrences and at least `min_chars` total characters are
/// returned, sorted by `total_chars DESC`.
pub fn top_noise_signatures_via_host(
    host: &dyn Host,
    min_occurrences: u64,
    min_chars: u64,
) -> ApiResult<Vec<crate::NoiseRow>> {
    let db = host.sqlite(SCHEMA_SQL, SensitivityClass::Public)?;
    let rows = db.query(
        "SELECT command, signature, sample, occurrences, total_chars
         FROM noise_signatures
         WHERE occurrences >= ?1 AND total_chars >= ?2
         ORDER BY total_chars DESC",
        &[
            serde_json::json!(min_occurrences as i64),
            serde_json::json!(min_chars as i64),
        ],
    )?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let command = row
                .first()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let signature = row
                .get(1)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sample = row
                .get(2)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let occurrences = row.get(3).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;
            let total_chars = row.get(4).and_then(|v| v.as_i64()).unwrap_or(0).max(0) as u64;
            crate::NoiseRow {
                command,
                signature,
                sample,
                occurrences,
                total_chars,
            }
        })
        .collect())
}

// ── Legacy migration ──────────────────────────────────────────────────────────

/// Fast check: does the legacy analytics.db still exist?
/// If not, we can skip the entire migration path in O(1).
fn legacy_db_exists() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".tkr").join("analytics.db").exists())
        .unwrap_or(false)
}

/// Import rows from `~/.tkr/analytics.db` into the vault sqlite, then rename
/// the file to `~/.tkr/analytics.db.migrated` to prevent double-migration.
fn migrate_legacy_db(host: &dyn Host) -> ApiResult<()> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Ok(()),
    };
    let legacy_path = home.join(".tkr").join("analytics.db");
    if !legacy_path.exists() {
        return Ok(());
    }

    // Open the legacy DB with rusqlite.
    let conn = match rusqlite::Connection::open(&legacy_path) {
        Ok(c) => c,
        Err(_) => return Ok(()), // unreadable — skip silently
    };

    // Collect all rows.
    let rows_result = {
        let mut stmt =
            match conn.prepare("SELECT command, chars_in, chars_saved, runs FROM command_stats") {
                Ok(s) => s,
                Err(_) => return Ok(()), // table may not exist
            };
        let rows: Vec<(String, i64, i64, i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .and_then(|iter| iter.collect::<rusqlite::Result<Vec<_>>>())
            .unwrap_or_default();
        rows
    };
    // Drop conn before renaming on some platforms.
    drop(conn);

    // Write into vault sqlite.
    if !rows_result.is_empty() {
        if let Ok(db) = host.sqlite(SCHEMA_SQL, SensitivityClass::Public) {
            for (cmd, chars_in, chars_saved, runs) in &rows_result {
                let n = (*runs).max(1);
                let per_run_chars_in = chars_in / n;
                let per_run_chars_saved = chars_saved / n;
                for _ in 0..*runs {
                    let _ = db.execute(
                        UPSERT_SQL,
                        &[
                            json!(cmd),
                            json!(per_run_chars_in),
                            json!(per_run_chars_saved),
                        ],
                    );
                }
            }
        }
    }

    // Migrate noise_signatures (filter-learning candidates) from the legacy DB.
    if let Ok(conn) = rusqlite::Connection::open(&legacy_path) {
        let sig_rows: Vec<(String, String, String, i64, i64, i64)> = match conn.prepare(
            "SELECT command, signature, sample, occurrences, total_chars, last_seen
                 FROM noise_signatures",
        ) {
            Ok(mut stmt) => stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                })
                .and_then(|iter| iter.collect::<rusqlite::Result<Vec<_>>>())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        if !sig_rows.is_empty() {
            if let Ok(db) = host.sqlite(SCHEMA_SQL, SensitivityClass::Public) {
                for (cmd, sig, sample, occ, chars, ts) in sig_rows {
                    let _ = db.execute(
                        NOISE_UPSERT_SQL,
                        &[
                            json!(cmd),
                            json!(sig),
                            json!(sample),
                            json!(occ),
                            json!(chars),
                            json!(ts),
                        ],
                    );
                }
            }
        }
    }

    // Rename the legacy file so we don't re-migrate on next startup.
    let migrated_path = home.join(".tkr").join("analytics.db.migrated");
    let _ = std::fs::rename(&legacy_path, &migrated_path);

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[cfg(feature = "test-host")]
    use tkr_api::plugin::CommandCtx;
    use tkr_api::plugin::Plugin;

    // Use TestHost from tkr-api (test-host feature) wrapped in Arc.
    // TestSqlite is a no-op (records calls, returns empty rows), which is
    // sufficient to verify the call contract without needing a real DB.
    #[cfg(feature = "test-host")]
    fn make_test_host() -> Arc<dyn Host + 'static> {
        Arc::new(tkr_api::test_host::TestHost::new("tkr-analytics"))
    }

    #[cfg(not(feature = "test-host"))]
    mod noop_host_stub {
        use std::sync::Arc;

        use tkr_api::bus::{Bus, Event, Reply, Request};
        use tkr_api::handles::{Fs, Kv, Sqlite};
        use tkr_api::host::Host;
        use tkr_api::manifest::SensitivityClass;
        use tkr_api::vault::{SealState, Vault};
        use tkr_api::{Error, Result as ApiResult};

        struct StubBus;

        impl Bus for StubBus {
            fn request(&self, req: Request) -> ApiResult<Reply> {
                Err(Error::UnknownMethod(req.method))
            }

            fn emit(&self, _evt: Event) -> ApiResult<()> {
                Ok(())
            }
        }

        struct StubVault;

        impl Vault for StubVault {
            fn state(&self) -> SealState {
                SealState::Sealed
            }

            fn read_secret(&self, _key: &str) -> ApiResult<Vec<u8>> {
                Err(Error::Vault("stub".into()))
            }

            fn write_secret(&self, _key: &str, _val: &[u8]) -> ApiResult<()> {
                Err(Error::Vault("stub".into()))
            }
        }

        struct NoopHost;

        static STUB_BUS: StubBus = StubBus;
        static STUB_VAULT: StubVault = StubVault;

        impl Host for NoopHost {
            fn plugin_name(&self) -> &str {
                "test"
            }

            fn bus(&self) -> &dyn Bus {
                &STUB_BUS
            }

            fn vault(&self) -> &dyn Vault {
                &STUB_VAULT
            }

            fn kv(&self, _: SensitivityClass) -> ApiResult<&dyn Kv> {
                Err(Error::Plugin("noop".into()))
            }

            fn sqlite(&self, _: &str, _: SensitivityClass) -> ApiResult<&dyn Sqlite> {
                Err(Error::Plugin("noop".into()))
            }

            fn fs(&self, _: SensitivityClass) -> ApiResult<&dyn Fs> {
                Err(Error::Plugin("noop".into()))
            }
        }

        pub(super) fn make_host() -> Arc<dyn Host + 'static> {
            Arc::new(NoopHost)
        }
    }

    #[cfg(feature = "test-host")]
    fn ctx(command: &str, args: &str) -> CommandCtx {
        CommandCtx {
            command: command.into(),
            args: args.into(),
            line_index: 0,
        }
    }

    /// Verify that on_command_end issues an UPSERT with the correct chars_in.
    /// Since TestSqlite is a no-op recorder, we inspect the recorded SQL calls.
    #[cfg(feature = "test-host")]
    #[test]
    fn aggregates_chars_in_per_command() {
        use tkr_api::test_host::TestHost;

        let host_inner = Arc::new(TestHost::new("tkr-analytics"));
        // Coerce Arc<TestHost> to Arc<dyn Host> for the plugin.
        let host: Arc<dyn Host + 'static> = host_inner.clone();
        let mut p = AnalyticsPluginV2::new();
        p.on_load(host).unwrap();

        // Simulate two lines for "git status".
        p.on_command_begin(&ctx("git", "status")).unwrap();
        // "line one" = 8 chars, "line two" = 8 chars => 16 total
        p.on_line("line one", &ctx("git", "status")).unwrap();
        p.on_line("line two", &ctx("git", "status")).unwrap();
        p.on_command_end(&ctx("git", "status")).unwrap();

        // Inspect the calls made to the public sqlite handle via the helper.
        let calls = host_inner.sqlite_pub_calls();
        // The UPSERT should have been made: find a call with "git status" as first param.
        let upsert_call = calls.iter().find(|(sql, params)| {
            sql.contains("INSERT INTO command_stats")
                && params.first() == Some(&serde_json::json!("git status"))
        });
        assert!(
            upsert_call.is_some(),
            "expected UPSERT for 'git status'; recorded calls: {:?}",
            calls
        );

        let (_sql, params) = upsert_call.unwrap();
        // params[1] = chars_in (16 = len("line one") + len("line two"))
        assert_eq!(params[1], serde_json::json!(16_i64));
    }

    /// Verify analytics.savings returns the rows from the sqlite query.
    /// With TestSqlite the query always returns empty, so we verify the reply
    /// structure (empty rows array) and that no error is returned.
    #[cfg(feature = "test-host")]
    #[test]
    fn analytics_savings_returns_reply() {
        let host: Arc<dyn Host + 'static> = make_test_host();
        let mut p = AnalyticsPluginV2::new();
        p.on_load(host).unwrap();

        let reply = p
            .on_request(Request {
                target: "tkr-analytics".into(),
                method: "analytics.savings".into(),
                payload: json!({}),
                caller: "test".into(),
            })
            .unwrap();

        // TestSqlite returns no rows; reply should be structurally valid.
        assert!(reply.payload["rows"].is_array());
    }

    /// on_request with an unknown method must return UnknownMethod.
    #[cfg(feature = "test-host")]
    #[test]
    fn analytics_savings_request_unknown_method_errors() {
        let host: Arc<dyn Host + 'static> = make_test_host();
        let mut p = AnalyticsPluginV2::new();
        p.on_load(host).unwrap();

        let err = p
            .on_request(Request {
                target: "tkr-analytics".into(),
                method: "bad.method".into(),
                payload: json!({}),
                caller: "test".into(),
            })
            .unwrap_err();
        assert!(matches!(err, Error::UnknownMethod(m) if m == "bad.method"));
    }

    /// Migration test: pre-populate a fake HOME with a legacy analytics.db,
    /// call on_load, and verify the legacy file is renamed.
    #[test]
    fn migrates_legacy_analytics_db() {
        use rusqlite::{params, Connection};
        use tempfile::tempdir;

        // Create a temp directory to act as HOME.
        let fake_home = tempdir().unwrap();
        let tkr_dir = fake_home.path().join(".tkr");
        std::fs::create_dir_all(&tkr_dir).unwrap();

        // Populate legacy DB with one row.
        let legacy_db_path = tkr_dir.join("analytics.db");
        {
            let conn = Connection::open(&legacy_db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS command_stats (
                    id          INTEGER PRIMARY KEY,
                    command     TEXT NOT NULL,
                    chars_in    INTEGER NOT NULL DEFAULT 0,
                    chars_saved INTEGER NOT NULL DEFAULT 0,
                    runs        INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(command)
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO command_stats (command, chars_in, chars_saved, runs) VALUES (?1, ?2, ?3, ?4)",
                params!["git status", 1000_i64, 800_i64, 5_i64],
            )
            .unwrap();
        }

        // Override HOME so migrate_legacy_db picks up the fake home.
        // SAFETY: this test must not run in parallel with other HOME-dependent tests.
        unsafe {
            std::env::set_var("HOME", fake_home.path());
        }

        // Use a minimal TestHost; migration outcome is visible via the file system.
        #[cfg(feature = "test-host")]
        let host: Arc<dyn Host + 'static> = make_test_host();
        #[cfg(not(feature = "test-host"))]
        let host: Arc<dyn Host + 'static> = noop_host_stub::make_host();

        let mut p = AnalyticsPluginV2::new();
        p.on_load(host).unwrap();

        // The legacy file should be renamed.
        assert!(
            !legacy_db_path.exists(),
            "legacy db should have been renamed after migration"
        );
        let migrated = tkr_dir.join("analytics.db.migrated");
        assert!(migrated.exists(), "migrated sentinel file should exist");

        unsafe {
            std::env::remove_var("HOME");
        }
    }

    /// `total_savings_via_host` must return Ok with an empty vec when the host
    /// sqlite has no rows (TestSqlite always returns empty query results).
    #[cfg(feature = "test-host")]
    #[test]
    fn total_savings_via_host_returns_empty_on_fresh_db() {
        let host: Arc<dyn Host + 'static> = make_test_host();
        let rows = super::total_savings_via_host(host.as_ref()).unwrap();
        assert!(
            rows.is_empty(),
            "expected empty rows from fresh db; got {:?}",
            rows.iter().map(|r| &r.command).collect::<Vec<_>>()
        );
    }

    /// `top_noise_signatures_via_host` must return Ok with an empty vec when
    /// the host sqlite has no rows.
    #[cfg(feature = "test-host")]
    #[test]
    fn top_noise_signatures_via_host_returns_empty_on_fresh_db() {
        let host: Arc<dyn Host + 'static> = make_test_host();
        let rows = super::top_noise_signatures_via_host(host.as_ref(), 5, 100).unwrap();
        assert!(
            rows.is_empty(),
            "expected empty noise rows; got {} rows",
            rows.len()
        );
    }
}
