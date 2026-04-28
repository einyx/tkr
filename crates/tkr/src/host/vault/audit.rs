use std::path::{Path, PathBuf};
use std::sync::Mutex;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

pub struct AuditLog {
    path: PathBuf,
    last_hash: Mutex<String>,
}

impl AuditLog {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let last = Self::compute_last_hash(&path)?;
        Ok(Self {
            path,
            last_hash: Mutex::new(last),
        })
    }

    pub fn open_temp() -> Self {
        let p = std::env::temp_dir()
            .join(format!("tkr-audit-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Self::open(&p).unwrap()
    }

    fn compute_last_hash(path: &Path) -> Result<String> {
        let s = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e.into()),
        };
        let mut last = String::new();
        for line in s.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: Entry = serde_json::from_str(line)?;
            last = entry.hash;
        }
        Ok(last)
    }

    pub fn append(&self, event: AuditEvent) -> Result<()> {
        let mut last = self.last_hash.lock().unwrap();
        let prev = last.clone();
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
        writeln!(f, "{line}")?;
        *last = hash;
        Ok(())
    }

    pub fn verify(&self) -> Result<bool> {
        let s = match std::fs::read_to_string(&self.path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(e) => return Err(e.into()),
        };
        let mut prev = String::new();
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
            prev = entry.hash;
        }
        Ok(true)
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
}
