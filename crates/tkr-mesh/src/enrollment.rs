//! HTTP enrollment: turn a verified invite + fresh identity into a
//! `JoinedMesh` record by calling the broker's `POST /join` endpoint.
//!
//! The broker's HTTP base URL is derived from the invite's `broker_url`
//! by swapping `wss://` → `https://` (and `ws://` → `http://` for dev).

use crate::{attestation::JoinAttestation, Error, Identity, Invite, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const JOIN_TIMEOUT_SECS: u64 = 10;

/// Local record persisted after a successful enrollment. The `secret_hex`
/// field carries the on-chain signing key — it is redacted from `Debug`
/// output and `Serialize`/`Deserialize` are not derived. Use `save()` /
/// `load()` to round-trip to disk; `save()` writes with mode 0o600 on Unix.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinedMesh {
    pub mesh_id: String,
    pub mesh_slug: String,
    pub broker_url: String,
    pub member_id: String,
    /// 0x-prefixed EIP-55 mesh address (== ethereum address of identity).
    pub address: String,
    /// 32-byte private key, hex. Treat as a secret.
    pub secret_hex: String,
}

impl std::fmt::Debug for JoinedMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinedMesh")
            .field("mesh_id", &self.mesh_id)
            .field("mesh_slug", &self.mesh_slug)
            .field("broker_url", &self.broker_url)
            .field("member_id", &self.member_id)
            .field("address", &self.address)
            .field("secret_hex", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize, Deserialize)]
struct JoinedMeshFile {
    mesh_id: String,
    mesh_slug: String,
    broker_url: String,
    member_id: String,
    address: String,
    secret_hex: String,
}

impl JoinedMesh {
    /// Persist this record to `path`. On Unix, the file is created with
    /// mode 0o600 (owner read/write only) before any bytes are written.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::Encoding(format!("create parent for {}: {e}", path.display())))?;
        }
        let file = JoinedMeshFile {
            mesh_id: self.mesh_id.clone(),
            mesh_slug: self.mesh_slug.clone(),
            broker_url: self.broker_url.clone(),
            member_id: self.member_id.clone(),
            address: self.address.clone(),
            secret_hex: self.secret_hex.clone(),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|e| Error::Encoding(format!("serialize JoinedMesh: {e}")))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| Error::Encoding(format!("open {}: {e}", path.display())))?;
            std::io::Write::write_all(&mut f, &json)
                .map_err(|e| Error::Encoding(format!("write {}: {e}", path.display())))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, &json)
                .map_err(|e| Error::Encoding(format!("write {}: {e}", path.display())))?;
        }
        Ok(())
    }

    /// Load a record previously written by `save()`.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| Error::Encoding(format!("read {}: {e}", path.display())))?;
        let file: JoinedMeshFile = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Encoding(format!("parse {}: {e}", path.display())))?;
        Ok(JoinedMesh {
            mesh_id: file.mesh_id,
            mesh_slug: file.mesh_slug,
            broker_url: file.broker_url,
            member_id: file.member_id,
            address: file.address,
            secret_hex: file.secret_hex,
        })
    }
}

/// Wire body for `POST /join`. The broker re-verifies the invite signature
/// against `invite_payload.owner` AND verifies the joiner's attestation
/// (proving they hold the private key for `address` and binding the
/// signature to this specific invite + a fresh timestamp).
#[derive(Debug, Serialize)]
struct JoinRequest<'a> {
    invite_token: &'a str,
    invite_payload: &'a Invite,
    address: String,
    join_attestation: &'a JoinAttestation,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct JoinResponse {
    ok: bool,
    #[serde(default)]
    member_id: Option<String>,
    #[serde(default, rename = "memberId")]
    member_id_camel: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl JoinResponse {
    fn member_id(&self) -> Option<&str> {
        self.member_id
            .as_deref()
            .or(self.member_id_camel.as_deref())
    }
}

/// Run the full enrollment flow:
/// 1. Verify the invite (signature + expiry) against `now`.
/// 2. POST `{ invite_token, invite_payload, address, display_name }` to the
///    broker's `/join`.
/// 3. Return a `JoinedMesh` record on 200 OK + `ok: true`.
///
/// `invite_token` should be the original base64url token the user pasted
/// (use `Invite::to_token()` to recover it after verifying parsed input —
/// the broker matches on the exact bytes it signed).
pub fn enroll(
    invite: &Invite,
    invite_token: &str,
    identity: &Identity,
    display_name: Option<&str>,
    now: u64,
) -> Result<JoinedMesh> {
    invite.verify(now)?;

    // Prove key control over `address` and bind the redemption to this
    // specific invite + a fresh timestamp. Convert the seconds-precision
    // `now` callers pass to ms (broker compares with `SystemTime::now()`).
    let attestation =
        JoinAttestation::issue(identity, &invite.mesh_id, invite_token, now.saturating_mul(1000));

    let join_url = http_join_url(&invite.broker_url)?;
    let body = JoinRequest {
        invite_token,
        invite_payload: invite,
        address: identity.address().to_checksum(),
        join_attestation: &attestation,
        display_name,
    };

    // ureq 3.x: AgentBuilder replaced by Agent::config_builder.
    // http_status_as_error(false) preserves the old behavior where a non-2xx
    // response is returned with body intact (we read it for error context).
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(JOIN_TIMEOUT_SECS)))
        .http_status_as_error(false)
        .build()
        .into();

    let mut request = agent
        .post(&join_url)
        .header("content-type", "application/json");
    // Brokers that gate /join behind a session cookie (e.g. tkr-server
    // post-hardening) accept the same TKR_MESH_WS_COOKIE used for the WS
    // upgrade. Forward it on enrollment too.
    if let Ok(cookie) = std::env::var("TKR_MESH_WS_COOKIE") {
        request = request.header("cookie", &format!("tkr_session={cookie}"));
    }
    let resp = request
        .send_json(serde_json::to_value(&body).map_err(|e| Error::Encoding(e.to_string()))?)
        .map_err(|e| Error::Encoding(format!("broker unreachable: {e}")))?;

    let status = resp.status().as_u16();
    let raw = resp
        .into_body()
        .read_to_string()
        .map_err(|e| Error::Encoding(format!("broker response read: {e}")))?;
    let parsed: JoinResponse = serde_json::from_str(&raw).map_err(|e| {
        let snippet = if raw.len() > 200 {
            format!("{}…", &raw[..200])
        } else {
            raw.clone()
        };
        Error::Encoding(format!(
            "broker response not JSON (status={status}): {e}; body={snippet}"
        ))
    })?;
    if !parsed.ok {
        return Err(Error::InvalidInvite(
            parsed.error.unwrap_or_else(|| "broker rejected join".into()),
        ));
    }
    let member_id = parsed
        .member_id()
        .ok_or_else(|| Error::Encoding("broker omitted member_id".into()))?
        .to_string();

    Ok(JoinedMesh {
        mesh_id: invite.mesh_id.clone(),
        mesh_slug: invite.mesh_slug.clone(),
        broker_url: invite.broker_url.clone(),
        member_id,
        address: identity.address().to_checksum(),
        secret_hex: hex::encode(identity.secret_bytes()),
    })
}

/// Map a `wss://host[:port]/path` broker URL to the HTTP enrollment URL.
/// Schemes `wss://` and `ws://` flip to `https://` and `http://`.
///
/// Path mapping:
/// - if the path ends with `/ws` (or `/ws/`), replace it with `/join`
///   (e.g. `wss://h/api/v1/mesh/ws` → `https://h/api/v1/mesh/join`).
/// - otherwise, append `/join` to host[:port] only — the
///   claudemesh-compatible legacy default.
fn http_join_url(broker_url: &str) -> Result<String> {
    let (scheme_out, rest) = if let Some(r) = broker_url.strip_prefix("wss://") {
        ("https", r)
    } else if let Some(r) = broker_url.strip_prefix("ws://") {
        ("http", r)
    } else if let Some(r) = broker_url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = broker_url.strip_prefix("http://") {
        ("http", r)
    } else {
        return Err(Error::Encoding(format!(
            "broker_url must use ws/wss/http/https scheme: {broker_url}"
        )));
    };

    // Trim trailing slash + query/fragment if any.
    let path_query_end = rest.find(|c: char| c == '?' || c == '#').unwrap_or(rest.len());
    let path_part = &rest[..path_query_end];
    let trimmed = path_part.trim_end_matches('/');

    let mapped = if let Some(prefix) = trimmed.strip_suffix("/ws") {
        format!("{prefix}/join")
    } else {
        let host = trimmed.split('/').next().unwrap_or(trimmed);
        if host.is_empty() {
            return Err(Error::Encoding("broker_url missing host".into()));
        }
        format!("{host}/join")
    };
    Ok(format!("{scheme_out}://{mapped}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Role;

    fn fixture_invite(broker_url: &str) -> (Invite, String, Identity) {
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_test",
            "test",
            broker_url,
            2_000_000_000,
            Role::Member,
        );
        let token = invite.to_token().unwrap();
        let joiner = Identity::generate();
        (invite, token, joiner)
    }

    #[test]
    fn http_join_url_swaps_wss_to_https() {
        assert_eq!(
            http_join_url("wss://broker.example.com/ws").unwrap(),
            "https://broker.example.com/join"
        );
        assert_eq!(
            http_join_url("ws://localhost:8080/ws").unwrap(),
            "http://localhost:8080/join"
        );
        assert_eq!(
            http_join_url("https://broker.example.com").unwrap(),
            "https://broker.example.com/join"
        );
    }

    #[test]
    fn http_join_url_preserves_path_prefix_when_replacing_ws() {
        // tkr-server serves at /api/v1/mesh/ws — the join endpoint is at
        // /api/v1/mesh/join (sibling, not at the host root).
        assert_eq!(
            http_join_url("wss://tkr.prysm.sh/api/v1/mesh/ws").unwrap(),
            "https://tkr.prysm.sh/api/v1/mesh/join"
        );
        assert_eq!(
            http_join_url("ws://localhost:4000/api/v1/mesh/ws/").unwrap(),
            "http://localhost:4000/api/v1/mesh/join"
        );
    }

    #[test]
    fn http_join_url_strips_query_and_fragment() {
        assert_eq!(
            http_join_url("wss://h/ws?foo=1").unwrap(),
            "https://h/join"
        );
        assert_eq!(
            http_join_url("wss://h/ws#frag").unwrap(),
            "https://h/join"
        );
    }

    #[test]
    fn http_join_url_rejects_bad_scheme() {
        assert!(http_join_url("ftp://broker/").is_err());
        assert!(http_join_url("broker.example/ws").is_err());
    }

    #[test]
    fn enroll_happy_path() {
        let mut server = mockito::Server::new();
        let broker_url = format!("http://{}/ws", server.host_with_port());
        let (invite, token, joiner) = fixture_invite(&broker_url);

        let mock = server
            .mock("POST", "/join")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"memberId":"member_abc"}"#)
            .create();

        let joined = enroll(&invite, &token, &joiner, Some("alice"), 1_700_000_000).unwrap();
        mock.assert();
        assert_eq!(joined.member_id, "member_abc");
        assert_eq!(joined.mesh_id, "mesh_test");
        assert_eq!(joined.address, joiner.address().to_checksum());
        assert_eq!(joined.secret_hex.len(), 64);
    }

    #[test]
    fn enroll_accepts_snake_case_member_id() {
        // Some broker impls may use snake_case; we accept both.
        let mut server = mockito::Server::new();
        let broker_url = format!("http://{}/ws", server.host_with_port());
        let (invite, token, joiner) = fixture_invite(&broker_url);

        server
            .mock("POST", "/join")
            .with_status(200)
            .with_body(r#"{"ok":true,"member_id":"snake_case_id"}"#)
            .create();

        let joined = enroll(&invite, &token, &joiner, None, 1_700_000_000).unwrap();
        assert_eq!(joined.member_id, "snake_case_id");
    }

    #[test]
    fn enroll_rejects_expired_invite_before_network() {
        let (invite, token, joiner) = fixture_invite("http://127.0.0.1:1/ws");
        // `now` past the invite's expiry.
        let err = enroll(&invite, &token, &joiner, None, 3_000_000_000).unwrap_err();
        assert!(matches!(err, Error::Expired { .. }));
    }

    #[test]
    fn enroll_propagates_broker_error_body() {
        let mut server = mockito::Server::new();
        let broker_url = format!("http://{}/ws", server.host_with_port());
        let (invite, token, joiner) = fixture_invite(&broker_url);

        server
            .mock("POST", "/join")
            .with_status(403)
            .with_body(r#"{"ok":false,"error":"invite revoked"}"#)
            .create();

        let err = enroll(&invite, &token, &joiner, None, 1_700_000_000).unwrap_err();
        match err {
            Error::InvalidInvite(msg) => assert!(msg.contains("invite revoked"), "msg: {msg}"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn enroll_rejects_response_without_member_id() {
        let mut server = mockito::Server::new();
        let broker_url = format!("http://{}/ws", server.host_with_port());
        let (invite, token, joiner) = fixture_invite(&broker_url);

        server
            .mock("POST", "/join")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create();

        let err = enroll(&invite, &token, &joiner, None, 1_700_000_000).unwrap_err();
        assert!(matches!(err, Error::Encoding(_)));
    }
}
