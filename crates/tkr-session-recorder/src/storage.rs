use serde::{Deserialize, Serialize};
use serde_json::json;
use tkr_api::{host::Host, manifest::SensitivityClass, Result as ApiResult};

/// One captured tool-call event. Schema matches the design spec §5.2.
///
/// Token counts are estimated (bytes / 4) in v1; mark `_estimated` in any UI
/// that surfaces them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: String,
    pub session_id: String,
    pub seq: u64,
    pub tool: String,
    pub input: serde_json::Value,
    pub output_preview: String,
    #[serde(default)]
    pub output_full_ref: Option<String>,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub filter_savings_tokens: u32,
    #[serde(default)]
    pub cache_hit: Option<bool>,
    pub duration_ms: u32,
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// Session metadata. One file per session at `sessions/<id>/meta.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    pub agent: String,
    #[serde(default)]
    pub project_root: Option<String>,
    pub tkr_version: String,
}

/// Append an event to `sessions/<session_id>/events.jsonl` in the vault FS.
///
/// v1 implementation: read the existing file (if any), append one line in
/// memory, write back. Acceptable for low-volume sessions (<1 MB). For larger
/// recordings, a streaming append API on `Fs` would be the right fix.
pub fn append_event(host: &dyn Host, event: &Event) -> ApiResult<()> {
    let fs = host.fs(SensitivityClass::Secret)?;
    let path = events_path(&event.session_id);

    let mut buf = match fs.read(&path) {
        Ok(b) => b,
        Err(_) => Vec::new(), // first event for this session
    };

    let line = serde_json::to_string(event)
        .map_err(|e| tkr_api::Error::Plugin(format!("event serialise: {e}")))?;
    if !buf.is_empty() && !buf.ends_with(b"\n") {
        buf.push(b'\n');
    }
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');

    fs.write(&path, &buf)
}

/// Write or update `sessions/<session_id>/meta.json`. Idempotent — callers
/// pass the full meta each time.
pub fn write_meta(host: &dyn Host, meta: &SessionMeta) -> ApiResult<()> {
    let fs = host.fs(SensitivityClass::Secret)?;
    let path = meta_path(&meta.session_id);
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| tkr_api::Error::Plugin(format!("meta serialise: {e}")))?;
    fs.write(&path, &bytes)
}

/// Read meta for a session, if it exists.
pub fn read_meta(host: &dyn Host, session_id: &str) -> ApiResult<Option<SessionMeta>> {
    let fs = host.fs(SensitivityClass::Secret)?;
    let path = meta_path(session_id);
    match fs.read(&path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| tkr_api::Error::Plugin(format!("meta parse: {e}"))),
        Err(_) => Ok(None),
    }
}

/// List all known session IDs (one entry per `sessions/<id>/meta.json`).
pub fn list_sessions(host: &dyn Host) -> ApiResult<Vec<String>> {
    let fs = host.fs(SensitivityClass::Secret)?;
    let entries = fs.list("sessions/")?;
    Ok(entries
        .into_iter()
        .filter_map(|e| {
            // Expect "sessions/<id>/meta.json"
            let rest = e.strip_prefix("sessions/")?;
            let id = rest.split('/').next()?;
            if rest.ends_with("/meta.json") {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect())
}

/// Read all events for a session in seq order. v1 reads and parses the whole
/// JSONL file; for large sessions a streaming reader would be the right fix.
pub fn read_events(host: &dyn Host, session_id: &str) -> ApiResult<Vec<Event>> {
    let fs = host.fs(SensitivityClass::Secret)?;
    let buf = match fs.read(&events_path(session_id)) {
        Ok(b) => b,
        Err(_) => return Ok(Vec::new()),
    };
    let text = std::str::from_utf8(&buf)
        .map_err(|e| tkr_api::Error::Plugin(format!("events utf8: {e}")))?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(ev) => out.push(ev),
            Err(_) => continue, // skip malformed lines rather than fail the whole read
        }
    }
    out.sort_by_key(|e| e.seq);
    Ok(out)
}

fn events_path(session_id: &str) -> String {
    format!("sessions/{session_id}/events.jsonl")
}

fn meta_path(session_id: &str) -> String {
    format!("sessions/{session_id}/meta.json")
}

/// Estimate tokens from a string. v1: bytes / 4. Documented as estimated.
pub fn estimate_tokens(s: &str) -> u32 {
    (s.len() / 4) as u32
}

/// Generate a new session ID. Format: `<unix_ms_hex>-<random_hex>` so IDs are
/// time-sortable and globally unique without pulling in `ulid`.
pub fn new_session_id() -> String {
    use rand::Rng;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let rand_part: u32 = rand::thread_rng().gen();
    format!("{ms:013x}-{rand_part:08x}")
}

/// Convenience: build an event with the given fields, filling in `ts` from now.
pub fn make_event(
    session_id: impl Into<String>,
    seq: u64,
    tool: impl Into<String>,
    input: serde_json::Value,
    output_preview: impl Into<String>,
    tokens_in: u32,
    tokens_out: u32,
    filter_savings_tokens: u32,
    duration_ms: u32,
    exit_code: Option<i32>,
) -> Event {
    Event {
        ts: chrono::Utc::now().to_rfc3339(),
        session_id: session_id.into(),
        seq,
        tool: tool.into(),
        input,
        output_preview: output_preview.into(),
        output_full_ref: None,
        tokens_in,
        tokens_out,
        filter_savings_tokens,
        cache_hit: None,
        duration_ms,
        exit_code,
    }
}

// Silence unused-import warnings under cfg-gating.
#[allow(dead_code)]
fn _json_use() -> serde_json::Value {
    json!({})
}
