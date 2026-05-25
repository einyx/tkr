use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub actor: String,
    pub op: String,
    pub key: String,
    pub ts: u64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    prev_hash: String,
    event: AuditEvent,
    hash: String,
}

#[derive(Serialize, Deserialize)]
struct Tip {
    hash: String,
    n: u64,
}

fn tip_path(log: &Path) -> PathBuf {
    // Replace the log extension with "tip", e.g. "a.log" -> "a.tip"
    log.with_extension("tip")
}

fn write_tip(log: &Path, tip: &Tip) -> Result<()> {
    let p = tip_path(log);
    let tmp = p.with_extension("tip.tmp");
    let json = serde_json::to_vec(tip)?;
    let mut f = std::fs::File::create(&tmp)?;
    use std::io::Write;
    f.write_all(&json)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

fn read_tip(log: &Path) -> Result<Option<Tip>> {
    match std::fs::read(tip_path(log)) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub struct AuditLog {
    path: PathBuf,
    /// (last_hash, entry_count)
    last_hash: Mutex<(String, u64)>,
}

impl AuditLog {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let (last, n) = Self::compute_last_hash(&path)?;

        // If file exists (n > 0), compare against tip file.
        if n > 0 {
            match read_tip(&path)? {
                None => {
                    // Entries exist but tip is missing — treat as corruption.
                    return Err(anyhow::anyhow!(
                        "audit log tip file missing but log has {n} entries; possible truncation"
                    ));
                }
                Some(tip) => {
                    if tip.hash != last || tip.n != n {
                        return Err(anyhow::anyhow!(
                            "audit log tip mismatch: tip says hash={} n={}, log has hash={} n={}",
                            tip.hash,
                            tip.n,
                            last,
                            n
                        ));
                    }
                }
            }
        }

        Ok(Self {
            path,
            last_hash: Mutex::new((last, n)),
        })
    }

    pub fn open_temp() -> Self {
        let p = std::env::temp_dir().join(format!("tkr-audit-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(tip_path(&p));
        Self::open(&p).unwrap()
    }

    /// Walk the log file, computing the last hash and entry count.
    /// If the final line fails to parse (torn write), it is silently skipped.
    fn compute_last_hash(path: &Path) -> Result<(String, u64)> {
        let s = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), 0)),
            Err(e) => return Err(e.into()),
        };
        let mut last = String::new();
        let mut n: u64 = 0;
        let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Entry>(line) {
                Ok(entry) => {
                    last = entry.hash;
                    n += 1;
                }
                Err(_) => {
                    // Torn tail: skip the last line if it's unparseable.
                    if i == lines.len() - 1 {
                        eprintln!("audit: warning: skipping torn tail entry at line {}", i + 1);
                    } else {
                        // Mid-file corruption — propagate as actual error.
                        return Err(anyhow::anyhow!("audit log parse error at line {}", i + 1));
                    }
                }
            }
        }
        Ok((last, n))
    }

    pub fn append(&self, event: AuditEvent) -> Result<()> {
        let mut state = self.last_hash.lock().unwrap();
        let prev = state.0.clone();
        let mut h = Sha256::new();
        h.update(prev.as_bytes());
        h.update(serde_json::to_vec(&event)?);
        let hash = hex::encode(h.finalize());
        let entry = Entry {
            prev_hash: prev,
            event,
            hash: hash.clone(),
        };
        let line = serde_json::to_string(&entry)?;
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        drop(f);

        let new_n = state.1 + 1;
        write_tip(
            &self.path,
            &Tip {
                hash: hash.clone(),
                n: new_n,
            },
        )?;
        *state = (hash, new_n);
        Ok(())
    }

    pub fn verify(&self) -> Result<bool> {
        let s = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // No log file. If tip also absent, that's fine (empty log).
                // If tip present, something is wrong.
                match read_tip(&self.path)? {
                    None => return Ok(true),
                    Some(_) => return Ok(false),
                }
            }
            Err(e) => return Err(e.into()),
        };

        let mut prev = String::new();
        let mut n: u64 = 0;
        let mut last_hash = String::new();
        for line in s.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => return Ok(false),
            };
            if entry.prev_hash != prev {
                return Ok(false);
            }
            let mut h = Sha256::new();
            h.update(entry.prev_hash.as_bytes());
            h.update(serde_json::to_vec(&entry.event)?);
            if hex::encode(h.finalize()) != entry.hash {
                return Ok(false);
            }
            prev = entry.hash.clone();
            last_hash = entry.hash;
            n += 1;
        }

        // Check tip file anchors the chain.
        match read_tip(&self.path)? {
            None => {
                // Entries exist but tip missing — truncation or deletion detected.
                if n > 0 {
                    return Ok(false);
                }
                // Empty log, no tip — fine.
                Ok(true)
            }
            Some(tip) => {
                if tip.n != n {
                    return Ok(false);
                }
                if tip.hash != last_hash {
                    return Ok(false);
                }
                Ok(true)
            }
        }
    }

    #[cfg(test)]
    pub fn tamper_for_test(&self) {
        if let Ok(s) = std::fs::read_to_string(&self.path) {
            if !s.is_empty() {
                let mut bytes = s.into_bytes();
                let len = bytes.len();
                if len > 1 {
                    bytes[len - 2] = bytes[len - 2].wrapping_add(1);
                }
                let _ = std::fs::write(&self.path, bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn audit_log_chain_detects_tampering() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("a.log")).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "read".into(),
            key: "k".into(),
            ts: 0,
        })
        .unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "write".into(),
            key: "k".into(),
            ts: 1,
        })
        .unwrap();
        assert!(log.verify().unwrap());
        log.tamper_for_test();
        assert!(!log.verify().unwrap());
    }

    #[test]
    fn audit_persists_across_reopen() {
        let d = tempdir().unwrap();
        let p = d.path().join("a.log");
        {
            let log = AuditLog::open(&p).unwrap();
            log.append(AuditEvent {
                actor: "p".into(),
                op: "x".into(),
                key: "k".into(),
                ts: 0,
            })
            .unwrap();
        }
        let log = AuditLog::open(&p).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "y".into(),
            key: "k".into(),
            ts: 1,
        })
        .unwrap();
        assert!(log.verify().unwrap());
    }

    #[test]
    fn truncation_detected() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("a.log")).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "x".into(),
            key: "k".into(),
            ts: 0,
        })
        .unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "y".into(),
            key: "k".into(),
            ts: 1,
        })
        .unwrap();
        assert!(log.verify().unwrap());
        // Truncate the last line:
        let path = d.path().join("a.log");
        let s = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        let truncated: String = lines[..lines.len() - 1].join("\n") + "\n";
        std::fs::write(&path, truncated).unwrap();
        // Without tip-file protection this would return Ok(true).
        assert!(!log.verify().unwrap());
    }

    #[test]
    fn missing_tip_with_entries_fails_verify() {
        let d = tempdir().unwrap();
        let log = AuditLog::open(d.path().join("a.log")).unwrap();
        log.append(AuditEvent {
            actor: "p".into(),
            op: "x".into(),
            key: "k".into(),
            ts: 0,
        })
        .unwrap();
        std::fs::remove_file(d.path().join("a.tip")).unwrap();
        assert!(!log.verify().unwrap());
    }
}
