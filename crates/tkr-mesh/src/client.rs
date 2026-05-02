//! WSS client loop. Owns one long-lived WebSocket connection to a broker;
//! send/recv flow over mpsc channels.
//!
//! API shape:
//! 1. `Client::connect(joined, identity)` opens the WSS, sends Hello, awaits Ack.
//! 2. `client.send_dm(to, pubkey, plaintext)` enqueues a sealed Send frame.
//! 3. `client.next()` yields incoming frames (Push, Ack, Error) in order.
//!
//! The reader and writer halves of the socket are split across two tasks
//! bridged by tokio mpsc channels — the standard pattern for tungstenite
//! async usage. A graceful shutdown is triggered by dropping the Client
//! (the write task exits when its rx is closed; the read task exits on
//! its next loop iteration).

use crate::frames::{Frame, Hello, HelloFields, SendFields};
use crate::{Address, Envelope, Error, Identity, JoinedMesh, Result};
use futures_util::{SinkExt, StreamExt};
use k256::PublicKey;
use rand::Rng;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::Message;

const HELLO_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_BUF: usize = 64;
const READ_BUF: usize = 256;

#[derive(Debug)]
pub struct Client {
    write_tx: mpsc::Sender<Frame>,
    read_rx: mpsc::Receiver<Frame>,
    address: Address,
}

impl Client {
    /// The peer's mesh address (== signed-Hello identity).
    pub fn address(&self) -> Address {
        self.address
    }

    /// Open a WebSocket to the broker, complete the Hello/Ack handshake,
    /// and spawn read/write tasks. Times out after 5s waiting for ack.
    ///
    /// If `TKR_MESH_WS_COOKIE` is set in the environment, its value is sent
    /// as a `Cookie: tkr_session=<value>` header on the upgrade request.
    /// This is required when connecting to brokers that gate the mesh WS
    /// upgrade behind an authenticated session (e.g. tkr-server).
    pub async fn connect(joined: &JoinedMesh, identity: Identity) -> Result<Self> {
        use tokio_tungstenite::tungstenite::handshake::client::generate_key;
        use tokio_tungstenite::tungstenite::http::Uri;

        let ws_stream = if let Ok(cookie) = std::env::var("TKR_MESH_WS_COOKIE") {
            let uri: Uri = joined
                .broker_url
                .parse()
                .map_err(|e| Error::Encoding(format!("broker uri: {e}")))?;
            let host = uri.host().ok_or_else(|| Error::Encoding("broker uri missing host".into()))?;
            let host_hdr = match uri.port_u16() {
                Some(p) => format!("{host}:{p}"),
                None => host.to_string(),
            };
            let req = tokio_tungstenite::tungstenite::http::Request::builder()
                .uri(&joined.broker_url)
                .header("Host", host_hdr)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header("Sec-WebSocket-Key", generate_key())
                .header("Cookie", format!("tkr_session={cookie}"))
                .body(())
                .map_err(|e| Error::Encoding(format!("build ws request: {e}")))?;
            let (s, _resp) = tokio_tungstenite::connect_async(req)
                .await
                .map_err(|e| Error::Encoding(format!("ws connect: {e}")))?;
            s
        } else {
            let (s, _resp) = tokio_tungstenite::connect_async(&joined.broker_url)
                .await
                .map_err(|e| Error::Encoding(format!("ws connect: {e}")))?;
            s
        };

        let (mut sink, mut stream) = ws_stream.split();

        // Construct + send Hello.
        let session_id = format!(
            "{}-{}",
            std::process::id(),
            now_ms(),
        );
        let hello = Hello::new(
            &identity,
            joined.mesh_id.clone(),
            session_id,
            now_ms(),
        );
        let hello_json = Frame::from(hello)
            .to_json()
            .map_err(|e| Error::Encoding(format!("hello encode: {e}")))?;
        sink.send(Message::Text(hello_json))
            .await
            .map_err(|e| Error::Encoding(format!("hello send: {e}")))?;

        // Await Ack (or Error) before declaring the connection open.
        let ack = timeout(HELLO_ACK_TIMEOUT, async {
            while let Some(msg) = stream.next().await {
                match msg.map_err(|e| Error::Encoding(format!("ws read: {e}")))? {
                    Message::Text(t) => return Frame::from_json(&t),
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Close(_) => {
                        return Err(Error::Encoding("broker closed before ack".into()))
                    }
                    Message::Frame(_) => continue,
                }
            }
            Err(Error::Encoding("ws stream ended before ack".into()))
        })
        .await
        .map_err(|_| Error::Encoding("hello/ack timed out (5s)".into()))??;

        match ack {
            Frame::Ack(_) => {}
            Frame::Error(e) => {
                return Err(Error::Encoding(format!(
                    "broker rejected hello: {} ({})",
                    e.message, e.code
                )))
            }
            other => {
                return Err(Error::Encoding(format!(
                    "expected ack, got {}",
                    other
                        .to_json()
                        .unwrap_or_else(|_| "<unencodable>".into())
                )))
            }
        }

        // Spawn read + write tasks bridged via mpsc.
        let (write_tx, mut write_rx) = mpsc::channel::<Frame>(WRITE_BUF);
        let (read_tx, read_rx) = mpsc::channel::<Frame>(READ_BUF);

        // Writer task: forwards Frames from caller → WS sink as Text.
        tokio::spawn(async move {
            while let Some(frame) = write_rx.recv().await {
                let json = match frame.to_json() {
                    Ok(j) => j,
                    Err(_) => continue, // skip unencodable frames; should never happen
                };
                if sink.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });

        // Reader task: forwards parsed Frames from WS → caller via read_tx.
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(msg) = msg else { break };
                match msg {
                    Message::Text(t) => {
                        if let Ok(frame) = Frame::from_json(&t) {
                            if read_tx.send(frame).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => continue,
                }
            }
        });

        Ok(Client {
            write_tx,
            read_rx,
            address: identity.address(),
        })
    }

    /// Encrypt `plaintext` for `(to, recipient_pubkey)` and enqueue the Send
    /// frame. Resolves once the frame is in the writer's queue, NOT once
    /// the broker has acked.
    pub async fn send_dm(
        &self,
        to: Address,
        recipient_pubkey: &PublicKey,
        plaintext: &[u8],
    ) -> Result<String> {
        let envelope = Envelope::seal(plaintext, recipient_pubkey, to)?;
        let id = random_id();
        let frame = Frame::Send(SendFields {
            id: id.clone(),
            to,
            priority: Default::default(),
            envelope,
        });
        self.write_tx
            .send(frame)
            .await
            .map_err(|_| Error::Encoding("client write channel closed".into()))?;
        Ok(id)
    }

    /// Send any pre-built Frame. Useful for Acks the caller wants to emit
    /// after processing a Push.
    pub async fn send_frame(&self, frame: Frame) -> Result<()> {
        self.write_tx
            .send(frame)
            .await
            .map_err(|_| Error::Encoding("client write channel closed".into()))
    }

    /// Pull the next frame from the broker. Returns `None` when the
    /// connection has closed.
    pub async fn next(&mut self) -> Option<Frame> {
        self.read_rx.recv().await
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn random_id() -> String {
    let bytes: [u8; 8] = rand::thread_rng().gen();
    hex::encode(bytes)
}

// Convenience: surface HelloFields construction so callers building their
// own Hello frames (e.g. brokers acting as members) don't need to reach
// into the frames module.
pub use crate::frames::HelloFields as RawHelloFields;
#[allow(dead_code)]
fn _hold(_h: HelloFields) {} // silence unused-import warning

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::{AckFields, Frame, PushFields};
    use k256::SecretKey;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn pubkey_of(id: &Identity) -> PublicKey {
        let sk = SecretKey::from_slice(&id.secret_bytes()).unwrap();
        sk.public_key()
    }

    /// Spin up a minimal in-process broker on a random port.
    /// Behavior: accept WS, expect a Hello, reply with Ack, then for each
    /// Send received emit a Push (forwarded back to the same client).
    async fn spawn_test_broker() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let ws = match accept_async(stream).await {
                        Ok(w) => w,
                        Err(_) => return,
                    };
                    let (mut sink, mut stream) = ws.split();

                    // Wait for Hello, reply with Ack.
                    let mut sender_addr: Option<Address> = None;
                    if let Some(Ok(Message::Text(t))) = stream.next().await {
                        if let Ok(Frame::Hello(h)) = Frame::from_json(&t) {
                            sender_addr = Some(h.address);
                            let ack = Frame::Ack(AckFields { id: h.session_id });
                            let _ = sink.send(Message::Text(ack.to_json().unwrap())).await;
                        }
                    }
                    let from = match sender_addr {
                        Some(a) => a,
                        None => return,
                    };

                    // For each Send, push it back as a Push from `from`.
                    while let Some(Ok(msg)) = stream.next().await {
                        if let Message::Text(t) = msg {
                            if let Ok(Frame::Send(s)) = Frame::from_json(&t) {
                                let push = Frame::Push(PushFields {
                                    id: s.id,
                                    from,
                                    envelope: s.envelope,
                                });
                                let _ = sink.send(Message::Text(push.to_json().unwrap())).await;
                            }
                        }
                    }
                });
            }
        });
        format!("ws://127.0.0.1:{port}/")
    }

    fn fake_joined(broker: &str, alice: &Identity) -> JoinedMesh {
        JoinedMesh {
            mesh_id: "mesh_test".into(),
            mesh_slug: "test".into(),
            broker_url: broker.to_string(),
            member_id: "member_alice".into(),
            address: alice.address().to_checksum(),
            secret_hex: hex::encode(alice.secret_bytes()),
        }
    }

    #[tokio::test]
    async fn connect_handshake_succeeds() {
        let broker = spawn_test_broker().await;
        let alice = Identity::generate();
        let joined = fake_joined(&broker, &alice);
        let _client = Client::connect(&joined, alice).await.expect("connect");
    }

    #[tokio::test]
    async fn round_trip_dm_via_loopback_broker() {
        let broker = spawn_test_broker().await;
        let alice = Identity::generate();
        let joined = fake_joined(&broker, &alice);
        let alice_pub = pubkey_of(&alice);

        let mut client = Client::connect(&joined, alice.clone()).await.expect("connect");

        // Alice sends a DM to herself (loopback broker reflects Send → Push).
        client
            .send_dm(alice.address(), &alice_pub, b"hello self")
            .await
            .expect("send");

        let frame = timeout(Duration::from_secs(2), client.next())
            .await
            .expect("recv timeout")
            .expect("connection closed");

        match frame {
            Frame::Push(p) => {
                assert_eq!(p.from, alice.address());
                let plaintext = p.envelope.open(&alice).expect("decrypt");
                assert_eq!(&plaintext, b"hello self");
            }
            other => panic!("expected Push, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_fails_when_broker_unreachable() {
        let alice = Identity::generate();
        let joined = JoinedMesh {
            mesh_id: "x".into(),
            mesh_slug: "x".into(),
            broker_url: "ws://127.0.0.1:1/".into(), // nothing listens here
            member_id: "x".into(),
            address: alice.address().to_checksum(),
            secret_hex: hex::encode(alice.secret_bytes()),
        };
        let err = Client::connect(&joined, alice).await.unwrap_err();
        assert!(matches!(err, Error::Encoding(_)), "got {err:?}");
    }
}
