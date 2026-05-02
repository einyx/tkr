//! tkr-mesh broker. Owned by tkr-server. Two surfaces:
//!
//! 1. `POST /api/v1/mesh/join` — HTTP enrollment. Caller posts
//!    `{ invite_token, invite_payload, address, display_name? }`. Broker
//!    verifies the invite (signature + expiry), records the member under
//!    the invite's `mesh_id`, returns `{ ok, memberId }`.
//!
//! 2. `GET /api/v1/mesh/ws` — WebSocket upgrade. Client sends a signed
//!    `Hello`; broker verifies the signature against the embedded
//!    `address`, looks up the corresponding member record, and starts
//!    routing. `Send` frames are forwarded as `Push` to the recipient
//!    address if connected (silently dropped if offline — store-and-
//!    forward is a v2 feature).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tkr_mesh::frames::{AckFields, ErrorFields, Frame, PushFields, HELLO_MAX_SKEW_MS};
use tkr_mesh::{Address, Invite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::WebSocketStream;

const PEER_BUF: usize = 128;

#[derive(Debug, Default)]
pub struct BrokerState {
    inner: Mutex<BrokerInner>,
}

#[derive(Debug, Default)]
struct BrokerInner {
    /// Per-mesh member registry. members[mesh_id][address] = MemberRecord.
    members: HashMap<String, HashMap<Address, MemberRecord>>,
    /// Currently-connected peers, keyed by mesh address. The mpsc sender
    /// pushes a Frame at the per-peer writer task.
    peers: HashMap<Address, mpsc::Sender<Frame>>,
}

#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub mesh_id: String,
    pub member_id: String,
    pub address: Address,
    pub display_name: Option<String>,
}

impl BrokerState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register a member after a successful invite verification.
    /// Returns the assigned `member_id`. If the address is already enrolled
    /// in the mesh, the existing record is reused.
    pub fn enroll(
        &self,
        mesh_id: &str,
        address: Address,
        display_name: Option<String>,
    ) -> MemberRecord {
        let mut inner = self.inner.lock().expect("broker lock");
        let mesh = inner
            .members
            .entry(mesh_id.to_string())
            .or_default();
        if let Some(existing) = mesh.get(&address) {
            return existing.clone();
        }
        let member_id = new_member_id();
        let record = MemberRecord {
            mesh_id: mesh_id.to_string(),
            member_id,
            address,
            display_name,
        };
        mesh.insert(address, record.clone());
        record
    }

    /// Look up a member by mesh + address. Used during the WS hello to
    /// confirm the connecting peer was previously enrolled.
    pub fn lookup(&self, mesh_id: &str, address: &Address) -> Option<MemberRecord> {
        self.inner
            .lock()
            .expect("broker lock")
            .members
            .get(mesh_id)
            .and_then(|m| m.get(address).cloned())
    }

    /// Register a connected peer's writer channel. Returns the previous
    /// sender if the peer was already connected (caller should drop it).
    pub fn attach_peer(&self, address: Address, tx: mpsc::Sender<Frame>) -> Option<mpsc::Sender<Frame>> {
        self.inner
            .lock()
            .expect("broker lock")
            .peers
            .insert(address, tx)
    }

    pub fn detach_peer(&self, address: &Address) {
        self.inner
            .lock()
            .expect("broker lock")
            .peers
            .remove(address);
    }

    /// Resolve a recipient address to its writer channel.
    pub fn route(&self, address: &Address) -> Option<mpsc::Sender<Frame>> {
        self.inner
            .lock()
            .expect("broker lock")
            .peers
            .get(address)
            .cloned()
    }

    /// Per-mesh status snapshot: enrolled member count + currently-connected
    /// peer count. Used by the dashboard's mesh panel.
    pub fn status(&self) -> BrokerStatus {
        let inner = self.inner.lock().expect("broker lock");
        let connected_addrs: std::collections::HashSet<&Address> = inner.peers.keys().collect();
        let mut meshes: Vec<MeshStatus> = inner
            .members
            .iter()
            .map(|(mesh_id, members)| MeshStatus {
                mesh_id: mesh_id.clone(),
                enrolled: members.len() as u64,
                connected: members
                    .keys()
                    .filter(|a| connected_addrs.contains(a))
                    .count() as u64,
            })
            .collect();
        meshes.sort_by(|a, b| a.mesh_id.cmp(&b.mesh_id));
        BrokerStatus {
            total_meshes: meshes.len() as u64,
            total_connected: inner.peers.len() as u64,
            meshes,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BrokerStatus {
    pub total_meshes: u64,
    pub total_connected: u64,
    pub meshes: Vec<MeshStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeshStatus {
    #[serde(rename = "meshId")]
    pub mesh_id: String,
    pub enrolled: u64,
    pub connected: u64,
}

// ---------- HTTP /join handler ----------

#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    pub invite_token: String,
    pub invite_payload: Invite,
    pub address: Address,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JoinResponse {
    pub ok: bool,
    #[serde(rename = "memberId")]
    pub member_id: String,
}

#[derive(Debug, Serialize)]
pub struct JoinError {
    pub ok: bool,
    pub error: String,
}

/// Handle the join HTTP body. Returns the response payload on success or
/// a (status, JSON) on failure for the caller to render.
pub fn handle_join(
    broker: &BrokerState,
    body: JoinRequest,
    now: u64,
) -> Result<JoinResponse, (u16, JoinError)> {
    if body.invite_payload.verify(now).is_err() {
        return Err((
            403,
            JoinError {
                ok: false,
                error: "invite verification failed".to_string(),
            },
        ));
    }
    // The invite_token is opaque to the broker but useful for audit logs;
    // we don't crack it open here — invite_payload is what we trust.
    let _ = body.invite_token;

    let record = broker.enroll(
        &body.invite_payload.mesh_id,
        body.address,
        body.display_name,
    );
    Ok(JoinResponse {
        ok: true,
        member_id: record.member_id,
    })
}

// ---------- WSS session ----------

/// Run a single client's WebSocket session. Caller has already upgraded
/// the HTTP connection; `ws` is the wrapped tungstenite stream.
///
/// Behavior:
/// 1. Wait for first frame; require it to be a signed Hello.
/// 2. Verify signature (ecrecover → must equal hello.address).
/// 3. Look up the member record; reject if not enrolled.
/// 4. Reply with Ack.
/// 5. Spawn a writer task fed from an mpsc<Frame> the broker pushes into.
/// 6. Read loop: for each Send, look up recipient, forward as Push.
pub async fn run_ws_session<S>(broker: Arc<BrokerState>, ws: WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();

    // Step 1-3: hello.
    let hello = match next_text_frame(&mut stream).await {
        Some(Frame::Hello(h)) => h,
        Some(other) => {
            let _ = send_error(&mut sink, "want_hello", "first frame must be hello", other_id(&other)).await;
            return;
        }
        None => return,
    };

    // Verify signature **and** freshness: re-build a Hello and call
    // verify_with_now(). A captured Hello older than HELLO_MAX_SKEW_MS
    // (or with a future-dated timestamp out of window) is rejected — this
    // makes captured frames non-replayable by a network adversary.
    let hello_check = tkr_mesh::frames::Hello {
        kind: tkr_mesh::frames::HelloTag::Hello,
        mesh_id: hello.mesh_id.clone(),
        address: hello.address,
        session_id: hello.session_id.clone(),
        timestamp_ms: hello.timestamp_ms,
        signature: hello.signature.clone(),
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if let Err(e) = hello_check.verify_with_now(now_ms, HELLO_MAX_SKEW_MS) {
        let code = match e {
            tkr_mesh::Error::BadSignature => "bad_signature",
            _ => "stale_hello",
        };
        let _ = send_error(&mut sink, code, "hello rejected", Some(hello.session_id.clone())).await;
        return;
    }

    // Step 3: peer must be a known member of the claimed mesh.
    let member = match broker.lookup(&hello.mesh_id, &hello.address) {
        Some(m) => m,
        None => {
            let _ = send_error(&mut sink, "not_a_member", "address not enrolled in mesh", Some(hello.session_id.clone())).await;
            return;
        }
    };

    // Step 4: Ack.
    let ack = Frame::Ack(AckFields { id: hello.session_id.clone() });
    if let Ok(json) = ack.to_json() {
        if sink.send(Message::Text(json)).await.is_err() {
            return;
        }
    }

    // Step 5: spawn writer task fed from an mpsc the broker enqueues into.
    let (peer_tx, mut peer_rx) = mpsc::channel::<Frame>(PEER_BUF);
    if let Some(prev) = broker.attach_peer(member.address, peer_tx.clone()) {
        // Older connection for the same peer — drop it (graceful handoff).
        drop(prev);
    }

    let writer = tokio::spawn(async move {
        while let Some(frame) = peer_rx.recv().await {
            let Ok(json) = frame.to_json() else { continue };
            if sink.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Step 6: read loop.
    while let Some(frame) = next_text_frame(&mut stream).await {
        match frame {
            Frame::Send(s) => {
                if let Some(target) = broker.route(&s.to) {
                    let push = Frame::Push(PushFields {
                        id: s.id,
                        from: member.address,
                        envelope: s.envelope,
                    });
                    let _ = target.send(push).await;
                }
                // recipient offline → silently drop (v2: store-and-forward)
            }
            Frame::Hello(_) => {
                // Re-hello on an open connection: ignore.
            }
            Frame::Ack(_) | Frame::Push(_) | Frame::Error(_) => {
                // Clients shouldn't send these; ignore.
            }
        }
    }

    // Cleanup.
    broker.detach_peer(&member.address);
    let _ = writer.await;
}

async fn next_text_frame<S>(
    stream: &mut futures_util::stream::SplitStream<WebSocketStream<S>>,
) -> Option<Frame>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(msg) = stream.next().await {
        match msg.ok()? {
            Message::Text(t) => return Frame::from_json(&t).ok(),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
    None
}

async fn send_error<S>(
    sink: &mut futures_util::stream::SplitSink<WebSocketStream<S>, Message>,
    code: &str,
    message: &str,
    id: Option<String>,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let frame = Frame::Error(ErrorFields {
        code: code.to_string(),
        message: message.to_string(),
        id,
    });
    if let Ok(json) = frame.to_json() {
        sink.send(Message::Text(json)).await?;
    }
    sink.close().await
}

fn other_id(f: &Frame) -> Option<String> {
    match f {
        Frame::Hello(h) => Some(h.session_id.clone()),
        Frame::Send(s) => Some(s.id.clone()),
        Frame::Ack(a) => Some(a.id.clone()),
        Frame::Push(p) => Some(p.id.clone()),
        Frame::Error(e) => e.id.clone(),
    }
}

fn new_member_id() -> String {
    use rand::Rng;
    let bytes: [u8; 8] = rand::thread_rng().gen();
    format!("member_{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tkr_mesh::{Identity, Role};

    #[test]
    fn enroll_idempotent() {
        let broker = BrokerState::new();
        let id = Identity::generate();
        let r1 = broker.enroll("mesh_x", id.address(), Some("alice".into()));
        let r2 = broker.enroll("mesh_x", id.address(), Some("ALICE".into()));
        assert_eq!(r1.member_id, r2.member_id, "re-enroll returns same id");
        assert_eq!(broker.lookup("mesh_x", &id.address()).unwrap().member_id, r1.member_id);
    }

    #[test]
    fn lookup_other_mesh_returns_none() {
        let broker = BrokerState::new();
        let id = Identity::generate();
        broker.enroll("mesh_a", id.address(), None);
        assert!(broker.lookup("mesh_b", &id.address()).is_none());
    }

    #[test]
    fn handle_join_happy_path() {
        let broker = BrokerState::new();
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_test",
            "test",
            "wss://broker.example/ws",
            2_000_000_000,
            Role::Member,
        );
        let token = invite.to_token().unwrap();
        let joiner = Identity::generate();
        let body = JoinRequest {
            invite_token: token,
            invite_payload: invite,
            address: joiner.address(),
            display_name: Some("alice".into()),
        };
        let resp = handle_join(&broker, body, 1_700_000_000).expect("ok");
        assert!(resp.ok);
        assert!(resp.member_id.starts_with("member_"));
        assert!(broker.lookup("mesh_test", &joiner.address()).is_some());
    }

    #[test]
    fn handle_join_rejects_expired_invite() {
        let broker = BrokerState::new();
        let owner = Identity::generate();
        let invite = Invite::issue(
            &owner,
            "mesh_test",
            "test",
            "wss://broker.example/ws",
            1_000,
            Role::Member,
        );
        let token = invite.to_token().unwrap();
        let joiner = Identity::generate();
        let body = JoinRequest {
            invite_token: token,
            invite_payload: invite,
            address: joiner.address(),
            display_name: None,
        };
        let err = handle_join(&broker, body, 2_000).unwrap_err();
        assert_eq!(err.0, 403);
    }
}
