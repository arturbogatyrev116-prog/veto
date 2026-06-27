mod common;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use messenger_crypto::{
    keys::{IdentityKeyPair, PreKeyBundle, SignedPreKey},
    ratchet::RatchetState,
    wire::{self, InitEnvelope},
    x3dh::{x3dh_receive, x3dh_send},
};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use x25519_dalek::StaticSecret;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build server routing frame: [4B: rid_len][rid][4B: msg_id][payload].
/// Tests pass msg_id=0 (no delivery ACK needed).
fn routing_frame(recipient_id: &str, payload: &[u8]) -> Vec<u8> {
    let id = recipient_id.as_bytes();
    let mut frame = Vec::with_capacity(4 + id.len() + 4 + payload.len());
    frame.extend_from_slice(&(id.len() as u32).to_be_bytes());
    frame.extend_from_slice(id);
    frame.extend_from_slice(&0u32.to_be_bytes()); // msg_id = 0 → no ACK
    frame.extend_from_slice(payload);
    frame
}

/// Strip the server-prepended `[4B sid_len][sid]` prefix and return the wire frame.
fn strip_sender_prefix(data: &bytes::Bytes) -> &[u8] {
    if data.len() < 4 { return &[]; }
    let sid_len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
    if data.len() < 4 + sid_len { return &[]; }
    &data[4 + sid_len..]
}

/// Skip text frames (presence/hello events) and return the next binary frame.
async fn next_binary<S>(ws: &mut S) -> bytes::Bytes
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match ws.next().await.expect("stream ended").expect("ws error") {
            WsMessage::Binary(b) => return b,
            WsMessage::Text(_)   => continue, // system event, skip
            other                => panic!("unexpected frame: {other:?}"),
        }
    }
}

fn encode_bundle(bundle: &PreKeyBundle) -> Vec<u8> {
    postcard::to_stdvec(bundle).expect("serialize PreKeyBundle")
}

fn decode_bundle(bytes: &[u8]) -> PreKeyBundle {
    postcard::from_bytes(bytes).expect("deserialize PreKeyBundle")
}

// ── Health ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200() {
    let (addr, _handle) = common::spawn_server().await;
    let resp = common::http_client().get(format!("http://{addr}/health")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["db"],   "ok");
    assert_eq!(body["nats"], "ok");
}

// ── Auth ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn register_returns_token() {
    let (addr, _handle) = common::spawn_server().await;
    let (user_id, token) = common::register(addr, "reg_test_user").await;
    assert!(!user_id.is_empty());
    assert_eq!(token.len(), 64, "token should be 64 hex chars (2×UUID simple)");
}

#[tokio::test]
async fn upload_prekeys_without_auth_is_401() {
    let (addr, _handle) = common::spawn_server().await;
    let (user_id, _token) = common::register(addr, "no_auth_user").await;

    let resp = common::http_client()
        .put(format!("http://{addr}/api/v1/users/{user_id}/prekeys"))
        .body(b"bytes".as_ref())
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status().as_u16(), 401);
}

#[tokio::test]
async fn upload_prekeys_wrong_user_is_403() {
    let (addr, _handle) = common::spawn_server().await;
    let (alice_id, alice_token) = common::register(addr, "alice_403").await;
    let (bob_id, _bob_token) = common::register(addr, "bob_403").await;

    // Alice's token, but Bob's user_id in the path → 403.
    let resp = common::http_client()
        .put(format!("http://{addr}/api/v1/users/{bob_id}/prekeys"))
        .header("Authorization", format!("Bearer {alice_token}"))
        .body(b"fakebundle".as_ref())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 403);

    // Alice can upload to her own slot → 204.
    let alice_identity = IdentityKeyPair::generate();
    let alice_spk = SignedPreKey::generate(1);
    let alice_bundle = PreKeyBundle {
        identity_key: alice_identity.dh_public(),
        identity_key_ed: alice_identity.public().verifying,
        signed_prekey: alice_spk.public(),
        signed_prekey_id: alice_spk.id,
        signed_prekey_sig: alice_spk.sign(&alice_identity),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: None,
        pq_spk_sig: None,
    };
    let resp = common::http_client()
        .put(format!("http://{addr}/api/v1/users/{alice_id}/prekeys"))
        .header("Authorization", format!("Bearer {alice_token}"))
        .body(encode_bundle(&alice_bundle))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 204);
}

// ── E2E crypto ────────────────────────────────────────────────────────────────

/// Full X3DH + Double Ratchet session routed through the live server.
#[tokio::test]
async fn alice_to_bob_happy_path() {
    let (addr, _handle) = common::spawn_server().await;

    let (alice_id, alice_token) = common::register(addr, "alice_happy").await;
    let (bob_id, bob_token) = common::register(addr, "bob_happy").await;

    // Bob generates keys and publishes his PreKeyBundle.
    let bob_identity = IdentityKeyPair::generate();
    let bob_spk = SignedPreKey::generate(1);
    let bob_spk_bytes = bob_spk.secret.to_bytes();
    let bob_bundle = PreKeyBundle {
        identity_key: bob_identity.dh_public(),
        identity_key_ed: bob_identity.public().verifying,
        signed_prekey: bob_spk.public(),
        signed_prekey_id: bob_spk.id,
        signed_prekey_sig: bob_spk.sign(&bob_identity),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: None,
        pq_spk_sig: None,
    };

    let client = common::http_client();
    client
        .put(format!("http://{addr}/api/v1/users/{bob_id}/prekeys"))
        .header("Authorization", format!("Bearer {bob_token}"))
        .body(encode_bundle(&bob_bundle))
        .send()
        .await
        .unwrap();

    // Alice fetches Bob's bundle and performs X3DH.
    let bytes = client
        .get(format!("http://{addr}/api/v1/users/{bob_id}/prekeys"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let fetched = decode_bundle(&bytes);

    let alice_identity = IdentityKeyPair::generate();
    let (alice_sk, x3dh_header, pq_ct) = x3dh_send(&alice_identity, &fetched).unwrap();
    let mut alice = RatchetState::init_alice(&alice_sk, fetched.signed_prekey);

    let bob_sk = x3dh_receive(&bob_identity, &bob_spk, None, &x3dh_header, pq_ct.as_deref(), None).unwrap();
    let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

    // Both connect via WebSocket.
    let mut alice_ws = common::ws_connect(addr, &alice_token).await;
    let mut bob_ws = common::ws_connect(addr, &bob_token).await;

    let ad = b"alice-bob-v1";

    // Alice sends InitEnvelope (first message includes X3DH header).
    let first_msg = alice.encrypt(b"Hello Bob!", ad);
    let init_env = InitEnvelope { x3dh: x3dh_header, message: first_msg, ad: ad.to_vec(), pq_ct };
    alice_ws
        .send(WsMessage::Binary(
            routing_frame(&bob_id, &wire::encode(&init_env).unwrap()).into(),
        ))
        .await
        .unwrap();

    let raw = timeout(Duration::from_secs(3), next_binary(&mut bob_ws))
        .await
        .expect("timeout on first message");
    let (recv_env, _): (InitEnvelope, _) = wire::decode(strip_sender_prefix(&raw)).unwrap();
    assert_eq!(bob.decrypt(&recv_env.message, &recv_env.ad).unwrap(), b"Hello Bob!");

    // Alice sends 2 more messages, Bob decrypts both.
    for i in 1u8..=2 {
        let pt = vec![i; 8];
        let msg = alice.encrypt(&pt, ad);
        alice_ws
            .send(WsMessage::Binary(
                routing_frame(&bob_id, &wire::encode(&msg).unwrap()).into(),
            ))
            .await
            .unwrap();

        let raw = timeout(Duration::from_secs(3), next_binary(&mut bob_ws))
            .await
            .expect("timeout");
        let (recv_msg, _): (messenger_crypto::ratchet::Message, _) = wire::decode(strip_sender_prefix(&raw)).unwrap();
        assert_eq!(bob.decrypt(&recv_msg, ad).unwrap(), pt);
    }

    // Bob replies — triggers DH ratchet on Alice's side.
    let bob_msg = bob.encrypt(b"Hi Alice!", ad);
    bob_ws
        .send(WsMessage::Binary(
            routing_frame(&alice_id, &wire::encode(&bob_msg).unwrap()).into(),
        ))
        .await
        .unwrap();

    let raw = timeout(Duration::from_secs(3), next_binary(&mut alice_ws))
        .await
        .expect("timeout");
    let (recv_msg, _): (messenger_crypto::ratchet::Message, _) = wire::decode(strip_sender_prefix(&raw)).unwrap();
    assert_eq!(alice.decrypt(&recv_msg, ad).unwrap(), b"Hi Alice!");
}

/// Message sent while recipient is offline is queued and delivered on reconnect.
#[tokio::test]
async fn offline_delivery() {
    let (addr, _handle) = common::spawn_server().await;

    let (_alice_id, alice_token) = common::register(addr, "alice_offline").await;
    let (bob_id, bob_token) = common::register(addr, "bob_offline").await;

    // Only Alice connects.
    let mut alice_ws = common::ws_connect(addr, &alice_token).await;

    let payload = b"queued_for_bob";
    alice_ws
        .send(WsMessage::Binary(routing_frame(&bob_id, payload).into()))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Bob connects — queued message should be delivered immediately.
    let mut bob_ws = common::ws_connect(addr, &bob_token).await;

    let received = timeout(Duration::from_secs(3), next_binary(&mut bob_ws))
        .await
        .expect("timeout waiting for offline message");
    assert_eq!(strip_sender_prefix(&received), payload);
}

/// The Double Ratchet consumes each message key once — replaying the same
/// Message must fail, even if it arrived over the network a second time.
#[tokio::test]
async fn replay_attack_rejected() {
    let alice_identity = IdentityKeyPair::generate();
    let bob_identity = IdentityKeyPair::generate();
    let bob_spk = SignedPreKey::generate(1);
    let bob_spk_bytes = bob_spk.secret.to_bytes();

    let bundle = PreKeyBundle {
        identity_key: bob_identity.dh_public(),
        identity_key_ed: bob_identity.public().verifying,
        signed_prekey: bob_spk.public(),
        signed_prekey_id: bob_spk.id,
        signed_prekey_sig: bob_spk.sign(&bob_identity),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: None,
        pq_spk_sig: None,
    };

    let (alice_sk, header, pq_ct) = x3dh_send(&alice_identity, &bundle).unwrap();
    let bob_sk = x3dh_receive(&bob_identity, &bob_spk, None, &header, pq_ct.as_deref(), None).unwrap();

    let mut alice = RatchetState::init_alice(&alice_sk, bundle.signed_prekey);
    let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

    let ad = b"replay-test";
    let msg = alice.encrypt(b"original", ad);

    assert_eq!(bob.decrypt(&msg, ad).unwrap(), b"original");

    // Same Message object replayed — key already consumed, must fail.
    assert!(bob.decrypt(&msg, ad).is_err(), "replay must be rejected");
}

/// 100 messages sent in rapid succession; Bob decrypts all in order.
#[tokio::test]
async fn concurrent_100_messages() {
    let (addr, _handle) = common::spawn_server().await;

    let (_alice_id, alice_token) = common::register(addr, "alice_100").await;
    let (bob_id, bob_token) = common::register(addr, "bob_100").await;

    // Set up X3DH.
    let bob_identity = IdentityKeyPair::generate();
    let bob_spk = SignedPreKey::generate(1);
    let bob_spk_bytes = bob_spk.secret.to_bytes();
    let bundle = PreKeyBundle {
        identity_key: bob_identity.dh_public(),
        identity_key_ed: bob_identity.public().verifying,
        signed_prekey: bob_spk.public(),
        signed_prekey_id: bob_spk.id,
        signed_prekey_sig: bob_spk.sign(&bob_identity),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: None,
        pq_spk_sig: None,
    };

    let client = common::http_client();
    client
        .put(format!("http://{addr}/api/v1/users/{bob_id}/prekeys"))
        .header("Authorization", format!("Bearer {bob_token}"))
        .body(encode_bundle(&bundle))
        .send()
        .await
        .unwrap();

    let fetched = decode_bundle(
        &client
            .get(format!("http://{addr}/api/v1/users/{bob_id}/prekeys"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    );

    let alice_identity = IdentityKeyPair::generate();
    let (alice_sk, x3dh_header, pq_ct) = x3dh_send(&alice_identity, &fetched).unwrap();
    let mut alice = RatchetState::init_alice(&alice_sk, fetched.signed_prekey);
    let bob_sk = x3dh_receive(&bob_identity, &bob_spk, None, &x3dh_header, pq_ct.as_deref(), None).unwrap();
    let mut bob = RatchetState::init_bob(&bob_sk, StaticSecret::from(bob_spk_bytes));

    let mut alice_ws = common::ws_connect(addr, &alice_token).await;
    let mut bob_ws = common::ws_connect(addr, &bob_token).await;

    let ad = b"bulk-100";
    const N: usize = 100;

    // First message: InitEnvelope.
    let mut expected: Vec<Vec<u8>> = Vec::with_capacity(N);
    let first_pt = b"msg_0".to_vec();
    let first_msg = alice.encrypt(&first_pt, ad);
    let init_env = InitEnvelope { x3dh: x3dh_header, message: first_msg, ad: ad.to_vec(), pq_ct };
    alice_ws
        .send(WsMessage::Binary(
            routing_frame(&bob_id, &wire::encode(&init_env).unwrap()).into(),
        ))
        .await
        .unwrap();
    expected.push(first_pt);

    // Messages 1..N: plain ratchet messages.
    for i in 1..N {
        let pt = format!("msg_{i}").into_bytes();
        let msg = alice.encrypt(&pt, ad);
        alice_ws
            .send(WsMessage::Binary(
                routing_frame(&bob_id, &wire::encode(&msg).unwrap()).into(),
            ))
            .await
            .unwrap();
        expected.push(pt);
    }

    // Bob receives and decrypts all N.
    for (i, expected_pt) in expected.iter().enumerate() {
        let payload = timeout(Duration::from_secs(10), next_binary(&mut bob_ws))
            .await
            .unwrap_or_else(|_| panic!("timeout on message {i}"));

        let wire = strip_sender_prefix(&payload);
        let decrypted = if i == 0 {
            let (env, _): (InitEnvelope, _) = wire::decode(wire).unwrap();
            bob.decrypt(&env.message, &env.ad).unwrap()
        } else {
            let (msg, _): (messenger_crypto::ratchet::Message, _) = wire::decode(wire).unwrap();
            bob.decrypt(&msg, ad).unwrap()
        };
        assert_eq!(&decrypted, expected_pt, "mismatch at message {i}");
    }
}
