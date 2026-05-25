//! Content-addressed index distribution.
//!
//! An *index bundle* is a SQLite DB compressed with gzip plus a small JSON
//! manifest describing what repo + commit it covers. The manifest itself is
//! hashed → its CID is what peers gossip.
//!
//! On-wire shape mirrors jkr-model::manifest. When jkr-model's iroh fetch
//! lands, swap [`BlobStore`] for the iroh-backed impl and announces flow
//! through the existing mesh registry pattern.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use crate::SCHEMA_VERSION;

/// Hex-encoded SHA-256. Mirrors [`jkr_model::manifest::Cid`]. Treat as opaque.
pub type Cid = String;

/// Bundle manifest — versioned, content-addressed description of an index.
///
/// Stability: once published, peers cache the CID forever. Add fields only
/// (never rename/remove); they must be `Option` or have a `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexManifest {
    /// Stable repo identifier. Free-form for v0 — origin URL, package name,
    /// whatever the publisher uses to mean "the same project."
    pub repo: String,
    /// Git commit the index was built against. None if not a git repo.
    pub commit: Option<String>,
    /// Schema version of the SQLite DB inside the bundle. Peers refuse
    /// bundles whose `schema_version != jkr_index::SCHEMA_VERSION`.
    pub schema_version: i32,
    /// CID of the gzipped DB blob.
    pub db_cid: Cid,
    /// Uncompressed size in bytes — lets a puller reject oversize bundles
    /// before fetching.
    pub db_size: u64,
    /// Unix seconds.
    pub created_at: i64,
}

impl IndexManifest {
    /// CID of the canonical JSON encoding of this manifest.
    pub fn cid(&self) -> Cid {
        let json = serde_json::to_vec(self).expect("manifest is serializable");
        hex::encode(Sha256::digest(&json))
    }
}

/// A place to put / fetch content-addressed blobs. Production impl: iroh.
/// V0 impl: local filesystem.
pub trait BlobStore {
    fn put(&mut self, cid: &str, bytes: &[u8]) -> Result<()>;
    fn get(&self, cid: &str) -> Result<Vec<u8>>;
}

/// In-memory store — for tests.
#[derive(Default)]
pub struct MemBlobStore(HashMap<String, Vec<u8>>);

impl BlobStore for MemBlobStore {
    fn put(&mut self, cid: &str, bytes: &[u8]) -> Result<()> {
        self.0.insert(cid.to_string(), bytes.to_vec());
        Ok(())
    }
    fn get(&self, cid: &str) -> Result<Vec<u8>> {
        self.0
            .get(cid)
            .cloned()
            .with_context(|| format!("blob {cid} not found"))
    }
}

/// Local-filesystem blob store. Each blob lives at `root/<cid>`.
pub struct LocalBlobStore {
    root: std::path::PathBuf,
}

impl LocalBlobStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root).context("create blob store dir")?;
        Ok(Self { root })
    }
}

impl BlobStore for LocalBlobStore {
    fn put(&mut self, cid: &str, bytes: &[u8]) -> Result<()> {
        std::fs::write(self.root.join(cid), bytes).context("write blob")
    }
    fn get(&self, cid: &str) -> Result<Vec<u8>> {
        std::fs::read(self.root.join(cid)).with_context(|| format!("read blob {cid}"))
    }
}

/// Bundle the index DB at `<repo_root>/.jkr/index.sqlite` and store it in
/// `store`. Returns the manifest CID — what peers gossip.
pub fn publish(
    repo_root: &Path,
    repo: impl Into<String>,
    commit: Option<String>,
    store: &mut dyn BlobStore,
) -> Result<Cid> {
    let db_path = repo_root.join(".jkr").join("index.sqlite");
    let db_bytes = std::fs::read(&db_path)
        .with_context(|| format!("read {}", db_path.display()))?;
    let uncompressed_size = db_bytes.len() as u64;

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&db_bytes)?;
    let compressed = gz.finish()?;
    let db_cid = hex::encode(Sha256::digest(&compressed));

    store.put(&db_cid, &compressed)?;

    let manifest = IndexManifest {
        repo: repo.into(),
        commit,
        schema_version: SCHEMA_VERSION,
        db_cid,
        db_size: uncompressed_size,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    let manifest_json = serde_json::to_vec(&manifest)?;
    let manifest_cid = manifest.cid();
    store.put(&manifest_cid, &manifest_json)?;
    Ok(manifest_cid)
}

/// Fetch a bundle by manifest CID, decompress, write the DB to
/// `<dest>/.jkr/index.sqlite`. Returns the manifest for caller inspection.
pub fn fetch(
    manifest_cid: &str,
    dest_repo_root: &Path,
    store: &dyn BlobStore,
) -> Result<IndexManifest> {
    let manifest_bytes = store.get(manifest_cid)?;
    let manifest: IndexManifest = serde_json::from_slice(&manifest_bytes)
        .context("manifest decode")?;
    if manifest.schema_version != SCHEMA_VERSION {
        anyhow::bail!(
            "schema mismatch: bundle={}, local={}",
            manifest.schema_version,
            SCHEMA_VERSION
        );
    }
    let compressed = store.get(&manifest.db_cid)?;
    // Verify content matches advertised CID — peers don't get to lie.
    let actual = hex::encode(Sha256::digest(&compressed));
    if actual != manifest.db_cid {
        anyhow::bail!("db CID mismatch: claimed={}, actual={}", manifest.db_cid, actual);
    }
    let mut gz = GzDecoder::new(&compressed[..]);
    let mut db_bytes = Vec::with_capacity(manifest.db_size as usize);
    gz.read_to_end(&mut db_bytes)?;

    let dir = dest_repo_root.join(".jkr");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("index.sqlite"), &db_bytes)?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IndexDb;

    #[test]
    fn round_trip_via_memstore() {
        // Build an index on repo A.
        let a = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("foo.rs"), "fn hello() {}\n").unwrap();
        let mut db = IndexDb::open(a.path()).unwrap();
        db.index_file(&a.path().join("foo.rs")).unwrap();
        drop(db);

        // Publish to in-memory store.
        let mut store = MemBlobStore::default();
        let cid = publish(
            a.path(),
            "github.com/example/repo",
            Some("deadbeef".to_string()),
            &mut store,
        )
        .unwrap();

        // Fetch into repo B and verify the index is queryable.
        let b = tempfile::tempdir().unwrap();
        let m = fetch(&cid, b.path(), &store).unwrap();
        assert_eq!(m.schema_version, SCHEMA_VERSION);
        assert_eq!(m.commit.as_deref(), Some("deadbeef"));

        let db_b = IndexDb::open(b.path()).unwrap();
        let n: i64 = db_b
            .conn()
            .query_row(
                "SELECT count(*) FROM symbols WHERE name='hello'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn fetch_rejects_tampered_blob() {
        let a = tempfile::tempdir().unwrap();
        std::fs::write(a.path().join("x.rs"), "fn x() {}\n").unwrap();
        let mut db = IndexDb::open(a.path()).unwrap();
        db.index_file(&a.path().join("x.rs")).unwrap();
        drop(db);

        let mut store = MemBlobStore::default();
        let cid = publish(a.path(), "r", None, &mut store).unwrap();
        // Corrupt the db blob behind the CID's back.
        let m: IndexManifest = serde_json::from_slice(&store.get(&cid).unwrap()).unwrap();
        store.0.insert(m.db_cid.clone(), b"junk".to_vec());

        let b = tempfile::tempdir().unwrap();
        let err = fetch(&cid, b.path(), &store).unwrap_err().to_string();
        assert!(err.contains("CID mismatch"), "got: {err}");
    }
}
