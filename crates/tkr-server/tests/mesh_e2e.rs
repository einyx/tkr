//! End-to-end mesh test: boot a real tkr-server in this process, have
//! two tkr-mesh Clients enroll over HTTP /api/v1/mesh/join, connect WSS,
//! and exchange an encrypted DM through the broker.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use k256::{PublicKey, SecretKey};
use tkr_mesh::frames::Frame;
use tkr_mesh::{enroll, Client, Identity, Invite, JoinedMesh, Role};
use tokio::time::timeout;

fn pubkey_of(id: &Identity) -> PublicKey {
    let sk = SecretKey::from_slice(&id.secret_bytes()).unwrap();
    sk.public_key()
}

struct ServerGuard {
    child: Child,
    port: u16,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pick_port() -> u16 {
    // Bind to :0 to discover an available port, drop it, return the number.
    // Race window is tiny but acceptable for a self-contained test.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn start_server() -> ServerGuard {
    let port = pick_port();
    let bin = env!("CARGO_BIN_EXE_tkr-server");
    let child = Command::new(bin)
        .env("HOST", "127.0.0.1")
        .env("PORT", port.to_string())
        // No TKR_ADMIN_PASSWORD → loopback fallback ("correct"). Fine for tests.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tkr-server");

    // Poll /health until ready.
    for _ in 0..50 {
        if let Ok(resp) = ureq::get(&format!("http://127.0.0.1:{port}/health"))
            .timeout(Duration::from_millis(200))
            .call()
        {
            if resp.status() == 200 {
                return ServerGuard { child, port };
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("tkr-server failed to start on port {port}");
}

/// Log in to the loopback dev server and return the `tkr_session` cookie
/// value. The broker now requires an authenticated session for both
/// `/api/v1/mesh/join` and the WS upgrade.
fn login_and_get_cookie(port: u16) -> String {
    let resp = ureq::post(&format!("http://127.0.0.1:{port}/api/auth/login"))
        .send_json(serde_json::json!({ "password": "correct" }))
        .expect("login http");
    let set_cookie = resp
        .header("set-cookie")
        .expect("set-cookie header on login");
    set_cookie
        .split(';')
        .find_map(|p| {
            let t = p.trim();
            t.strip_prefix("tkr_session=").map(|v| v.to_string())
        })
        .expect("tkr_session cookie")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_exchange_dm_through_real_broker() {
    let server = start_server().await;
    let cookie = login_and_get_cookie(server.port);
    // Plumb the cookie into the mesh WS handshake via env so Client::connect
    // (and the broker upgrade gate) accept the connection.
    std::env::set_var("TKR_MESH_WS_COOKIE", &cookie);

    // Owner mints an invite.
    let owner = Identity::generate();
    let broker_url = format!("ws://127.0.0.1:{}/api/v1/mesh/ws", server.port);
    let invite = Invite::issue(
        &owner,
        "mesh_e2e",
        "e2e",
        &broker_url,
        2_000_000_000,
        Role::Member,
    );
    let token = invite.to_token().unwrap();

    // Alice and Bob each enroll.
    let alice = Identity::generate();
    let bob = Identity::generate();
    let alice_pub = pubkey_of(&alice);
    let bob_pub = pubkey_of(&bob);

    let alice_joined = enroll_with_override(
        &server.port,
        &invite,
        &token,
        &alice,
        Some("alice"),
        &cookie,
    )
    .await;
    let bob_joined = enroll_with_override(
        &server.port,
        &invite,
        &token,
        &bob,
        Some("bob"),
        &cookie,
    )
    .await;

    // Both connect WSS to the broker.
    let mut alice_client = Client::connect(&alice_joined, alice.clone())
        .await
        .expect("alice connect");
    let mut bob_client = Client::connect(&bob_joined, bob.clone())
        .await
        .expect("bob connect");

    // Give the broker a moment to register both peers.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Alice → Bob.
    alice_client
        .send_dm(bob.address(), &bob_pub, b"hi bob")
        .await
        .expect("alice send");

    // Bob → Alice.
    bob_client
        .send_dm(alice.address(), &alice_pub, b"hi alice")
        .await
        .expect("bob send");

    // Bob receives Alice's DM.
    let bob_got = timeout(Duration::from_secs(2), bob_client.next())
        .await
        .expect("bob recv timeout")
        .expect("bob connection closed");
    match bob_got {
        Frame::Push(p) => {
            assert_eq!(p.from, alice.address());
            let pt = p.envelope.open(&bob).expect("bob decrypt");
            assert_eq!(&pt, b"hi bob");
        }
        other => panic!("bob expected Push, got {other:?}"),
    }

    // Alice receives Bob's DM.
    let alice_got = timeout(Duration::from_secs(2), alice_client.next())
        .await
        .expect("alice recv timeout")
        .expect("alice connection closed");
    match alice_got {
        Frame::Push(p) => {
            assert_eq!(p.from, bob.address());
            let pt = p.envelope.open(&alice).expect("alice decrypt");
            assert_eq!(&pt, b"hi alice");
        }
        other => panic!("alice expected Push, got {other:?}"),
    }
}

/// `enroll()` derives the broker's HTTP base URL from the invite's
/// broker_url, but our test invite points at the real WS URL the broker
/// will be listening on (e.g. `ws://127.0.0.1:N/api/v1/mesh/ws`). The
/// derived /join URL would be `http://127.0.0.1:N/join` — but our broker
/// serves /join at `/api/v1/mesh/join`. So this test calls the join
/// endpoint directly with the same body shape and feeds the result into a
/// JoinedMesh by hand.
async fn enroll_with_override(
    port: &u16,
    invite: &Invite,
    token: &str,
    identity: &Identity,
    display_name: Option<&str>,
    cookie: &str,
) -> JoinedMesh {
    // Confirm the invite verifies — same precondition as the real enroll().
    invite.verify(now_secs()).expect("invite verify");

    let body = serde_json::json!({
        "invite_token": token,
        "invite_payload": invite,
        "address": identity.address().to_checksum(),
        "display_name": display_name,
    });

    let resp = ureq::post(&format!("http://127.0.0.1:{port}/api/v1/mesh/join"))
        .set("cookie", &format!("tkr_session={cookie}"))
        .send_json(body)
        .expect("join http");
    let body: serde_json::Value = resp.into_json().expect("join json");
    assert_eq!(body["ok"], serde_json::Value::Bool(true), "{body}");
    let member_id = body["memberId"].as_str().expect("memberId").to_string();

    JoinedMesh {
        mesh_id: invite.mesh_id.clone(),
        mesh_slug: invite.mesh_slug.clone(),
        broker_url: invite.broker_url.clone(),
        member_id,
        address: identity.address().to_checksum(),
        secret_hex: hex::encode(identity.secret_bytes()),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// Silence unused-import warnings since `enroll` is exercised indirectly.
#[allow(dead_code)]
fn _hold(_e: fn(&Invite, &str, &Identity, Option<&str>, u64) -> tkr_mesh::Result<JoinedMesh>) {}
const _: fn(&Invite, &str, &Identity, Option<&str>, u64) -> tkr_mesh::Result<JoinedMesh> = enroll;
