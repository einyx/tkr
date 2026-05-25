//! File-system watcher that keeps the index fresh.
//!
//! Spawns a background thread that owns an [`IndexDb`] and a debounced
//! notify watcher. On a batch of file events it re-indexes only the files
//! that actually changed (the content-hash freshness check makes spurious
//! events cheap).
//!
//! The returned [`WatcherHandle`] must be kept alive by the caller; dropping
//! it stops the watcher.

use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::IndexDb;

/// Owns the watcher + worker thread. Drop to stop.
pub struct WatcherHandle {
    _debouncer: Debouncer<notify::RecommendedWatcher, FileIdMap>,
    // Worker thread joins on drop via the channel close.
    _join: Option<JoinHandle<()>>,
}

/// Start watching `repo_root` for file changes. The DB at `<root>/.jkr/`
/// is re-opened inside the worker thread so the caller doesn't hold a lock.
///
/// Debounced at 500ms — short enough to feel live, long enough to coalesce
/// editor save bursts.
pub fn start(repo_root: impl AsRef<Path>) -> Result<WatcherHandle> {
    let repo_root = repo_root.as_ref().to_path_buf();
    let (tx, rx) = mpsc::channel::<DebounceEventResult>();

    let mut debouncer = new_debouncer(Duration::from_millis(500), None, tx)
        .context("create debouncer")?;
    debouncer
        .watcher()
        .watch(&repo_root, RecursiveMode::Recursive)
        .context("attach watcher")?;

    let worker_root = repo_root.clone();
    let join = std::thread::spawn(move || {
        let mut db = match IndexDb::open(&worker_root) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[jkr-index watch] open db failed: {e}");
                return;
            }
        };
        while let Ok(events) = rx.recv() {
            let events = match events {
                Ok(e) => e,
                Err(errs) => {
                    eprintln!("[jkr-index watch] notify errors: {errs:?}");
                    continue;
                }
            };
            // Dedup paths in this batch.
            let mut touched: Vec<PathBuf> = Vec::new();
            for ev in events {
                for p in ev.event.paths {
                    if !touched.iter().any(|q| q == &p) {
                        touched.push(p);
                    }
                }
            }
            for p in touched {
                if should_skip(&p) {
                    continue;
                }
                if !p.is_file() {
                    continue;
                }
                if let Err(e) = db.index_file(&p) {
                    eprintln!("[jkr-index watch] index {} failed: {e}", p.display());
                }
            }
        }
    });

    Ok(WatcherHandle {
        _debouncer: debouncer,
        _join: Some(join),
    })
}

fn should_skip(p: &Path) -> bool {
    // Don't recurse into our own state dir or hidden vcs/build dirs — those
    // produce massive event storms during builds and rebases.
    p.components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some(".jkr" | ".git" | "target" | "node_modules" | ".venv")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_reindexes_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        // Seed an empty index so the worker has somewhere to write.
        {
            let _ = IndexDb::open(tmp.path()).unwrap();
        }
        let _h = start(tmp.path()).unwrap();

        // Give the watcher a moment to attach.
        std::thread::sleep(Duration::from_millis(100));

        let p = tmp.path().join("hello.rs");
        std::fs::write(&p, "fn greet() {}\n").unwrap();

        // Debounce window + processing slack.
        std::thread::sleep(Duration::from_millis(1200));

        let db = IndexDb::open(tmp.path()).unwrap();
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM symbols WHERE name='greet'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "watcher did not pick up new file");
    }
}
