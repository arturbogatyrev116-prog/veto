use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Key, Nonce};
use ed25519_dalek::SigningKey;
use native_tls;
use rand::RngCore;
use std::sync::atomic::Ordering;
use messenger_crypto::{
    keys::{self, IdentityKeyPair, OneTimePreKey, PreKeyBundle, SignedPreKey},
    ratchet::RatchetState,
    wire::{self, InitEnvelope, MessageEnvelope},
    x3dh::x3dh_send,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async_tls_with_config;

use crate::{
    client, db,
    store::{self, StoredIdentity},
    AppState, IdentityState, PeerSession, AD,
};

// ── Shared types ──────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
}

/// Reply metadata included when a message is a reply to another message.
#[derive(serde::Deserialize)]
pub struct ReplyInfo {
    pub ts: i64,
    pub from: String,
    pub text: String,
}

/// Returned by send_file so the frontend can display the download button immediately.
#[derive(Serialize)]
pub struct FileSentResult {
    pub id: u32,
    pub file_id: String,
    pub file_key: Vec<u8>,
}

/// Returned by send_group_file so the frontend can display the download button immediately.
#[derive(Serialize)]
pub struct GroupFileSentResult {
    pub file_id: String,
    pub file_key: Vec<u8>,
}

/// A single message as returned to the frontend for history display.
#[derive(Serialize)]
pub struct MessageView {
    /// SQLite row id — used as a cursor for pagination ("load more").
    pub db_id: i64,
    pub from: String,
    pub text: String,
    pub ts: i64,
    /// Session msg_id (for delivery-status correlation); None for received messages.
    pub id: Option<u32>,
    /// "sent" or "delivered"
    pub status: String,
    pub reply_to_ts: Option<i64>,
    pub reply_to_from: Option<String>,
    pub reply_to_text: Option<String>,
    pub file_id: Option<String>,
    pub file_key: Option<Vec<u8>>,
    pub file_name: Option<String>,
    pub file_mime: Option<String>,
    pub file_size: Option<i64>,
    /// Plaintext thumbnail bytes (JPEG, 200×200) stored locally — None for non-image or old messages.
    pub thumb_data: Option<Vec<u8>>,
    /// Set for group messages; the group UUID this message belongs to.
    pub group_id: Option<String>,
    /// For group messages: the sender's user_id (needed to show "from" in group view).
    pub sender_id: Option<String>,
}

/// Encode the wire payload as JSON v1, always including the sender's timestamp.
/// The ts field lets the receiver use the canonical timestamp for edit/reaction lookup.
fn encode_payload(text: &str, ts: i64, reply_to: Option<&ReplyInfo>) -> String {
    let mut obj = serde_json::json!({ "v": 1, "text": text, "ts": ts });
    if let Some(r) = reply_to {
        let quoted: String = if r.text.chars().count() > 100 {
            r.text.chars().take(100).collect::<String>() + "..."
        } else {
            r.text.clone()
        };
        obj["r"] = serde_json::json!({ "ts": r.ts, "f": r.from, "t": quoted });
    }
    obj.to_string()
}

// ── load_identity ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn load_identity(app: AppHandle) -> Result<Option<UserInfo>, String> {
    let data_dir = store::data_dir(&app)?;
    let path = data_dir.join("identity.json");

    let Some(stored) = store::load(&path).await else {
        return Ok(None);
    };

    populate_app_state(&app, &stored);

    Ok(Some(UserInfo { user_id: stored.user_id, username: stored.username }))
}

// ── register ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn register(username: String, app: AppHandle) -> Result<UserInfo, String> {
    let state = app.state::<AppState>();

    #[derive(serde::Deserialize)]
    struct Resp { user_id: String, token: String }

    let data_dir = store::data_dir(&app)?;
    let device_id = store::get_or_create_device_id(&data_dir);
    let device_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Desktop".to_string());

    let resp: Resp = state
        .http
        .post(format!("{}/api/v1/register", state.server_url))
        .json(&serde_json::json!({
            "username": username,
            "device_name": device_name,
            "device_id": device_id,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    // Generate identity key, signed prekey, and ML-KEM-768 post-quantum SPK.
    let keypair = IdentityKeyPair::generate();
    let spk = SignedPreKey::generate(1);
    let (pq_ek_bytes, pq_dk_bytes) = messenger_crypto::keys::generate_pq_spk();
    let pq_spk_sig: Vec<u8> = keypair.sign(&pq_ek_bytes).to_vec();

    let signing_key_bytes = keypair.signing.to_bytes();
    let spk_secret_bytes: [u8; 32] = spk.secret.to_bytes();
    let spk_id = spk.id;

    // Upload prekey bundle (raw postcard bytes)
    let bundle = PreKeyBundle {
        identity_key: keypair.dh_public(),
        identity_key_ed: keypair.public().verifying,
        signed_prekey: spk.public(),
        signed_prekey_id: spk.id,
        signed_prekey_sig: spk.sign(&keypair),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: Some(pq_ek_bytes),
        pq_spk_sig: Some(pq_spk_sig),
    };
    let bundle_bytes = postcard::to_stdvec(&bundle).map_err(|e| e.to_string())?;

    state
        .http
        .put(format!("{}/api/v1/users/{}/prekeys", state.server_url, resp.user_id))
        .header("Authorization", format!("Bearer {}", resp.token))
        .header("Content-Type", "application/octet-stream")
        .body(bundle_bytes)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    // Generate initial OPK batch and upload public keys to server.
    const INITIAL_OPK_COUNT: u32 = 10;
    let mut opk_secrets: Vec<(u32, [u8; 32])> = Vec::new();
    let mut opk_pub_keys: Vec<(u32, [u8; 32])> = Vec::new();
    for i in 0..INITIAL_OPK_COUNT {
        let opk = OneTimePreKey::generate(i);
        opk_pub_keys.push((i, opk.public().to_bytes()));
        opk_secrets.push((i, opk.secret.to_bytes()));
    }
    let opk_body = postcard::to_stdvec(&opk_pub_keys).map_err(|e| e.to_string())?;
    state
        .http
        .post(format!("{}/api/v1/users/{}/opks", state.server_url, resp.user_id))
        .header("Authorization", format!("Bearer {}", resp.token))
        .header("Content-Type", "application/octet-stream")
        .body(opk_body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    // Persist and activate
    let stored = StoredIdentity {
        user_id: resp.user_id.clone(),
        username: username.clone(),
        token: resp.token,
        signing_key: signing_key_bytes,
        spk_secret: spk_secret_bytes,
        spk_rotation_ts: store::now_ms(),
        spk_id,
        opk_secrets,
        opk_next_id: INITIAL_OPK_COUNT,
        pq_spk_secret: pq_dk_bytes,
    };

    store::save(&data_dir.join("identity.json"), &stored)
        .await
        .map_err(|e| e.to_string())?;

    populate_app_state(&app, &stored);

    Ok(UserInfo { user_id: resp.user_id, username })
}

// ── connect ───────────────────────────────────────────────────────────────────

// ── unlock ────────────────────────────────────────────────────────────────────

/// Derive the session key from the user's password (Argon2id) and load any
/// persisted sessions. Must be called before `connect`.
///
/// Returns `true` if previously-saved sessions were restored, `false` on first launch.
/// Returns `Err("incorrect_password")` if sessions.bin exists but decryption fails.
#[tauri::command]
pub async fn unlock(password: String, app: AppHandle) -> Result<bool, String> {
    let dir = store::data_dir(&app)?;

    // Argon2id is CPU-heavy (~300 ms) — run on blocking thread so we don't stall the runtime.
    let salt = store::load_or_create_salt(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let key = tokio::task::spawn_blocking(move || store::derive_session_key(&password, &salt))
        .await
        .map_err(|e| e.to_string())?;

    let sessions = store::load_sessions(&dir, &key)
        .await
        .ok_or_else(|| "incorrect_password".to_string())?;

    let restored = !sessions.is_empty();
    let state = app.state::<AppState>();
    {
        let mut sess = state.sessions.lock().unwrap();
        for (peer_id, session) in sessions {
            sess.entry(peer_id).or_insert(session);
        }
    }
    *state.session_key.lock().unwrap() = Some(key);

    // Open message history DB (created if it doesn't exist yet).
    let conn = db::open(&dir)?;
    // Initialise msg counter above the highest smid stored so cross-restart
    // collisions cannot cause a stale ACK to update the wrong DB row.
    let max = db::max_smid(&conn);
    state.msg_counter.store(max + 1, Ordering::Relaxed);

    // One-time FTS backfill: fill `plain` for messages stored before this migration.
    // Runs before putting `conn` into state — exclusive access, no locking needed.
    // Idempotent: skips rows that already have `plain` set.
    let key_bf = key;
    db::backfill_fts(&conn, move |nonce, ct| store::decrypt_content(nonce, ct, &key_bf).ok());

    *state.db.lock().unwrap() = Some(conn);

    // Background TTL cleanup — runs every 5 minutes while the app is open.
    {
        let app2 = app.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                let now = store::now_ms();
                let state = app2.state::<AppState>();
                let db_guard = state.db.lock().unwrap();
                if let Some(ref conn) = *db_guard {
                    db::purge_expired(conn, now);
                }
            }
        });
    }

    Ok(restored)
}

// ── connect ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn connect(app: AppHandle) -> Result<(), String> {
    let ws_url = {
        let state = app.state::<AppState>();
        let guard = state.identity.lock().unwrap();
        let id = guard.as_ref().ok_or("not registered")?;
        let base = state
            .server_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{base}/ws?token={}", id.token)
    };

    // Build a TLS connector; accept_invalid_certs=true allows self-signed certs (LAN dev).
    // For plain ws:// the connector is ignored by connect_async_tls_with_config.
    let accept_invalid = app.state::<AppState>().accept_invalid_certs;
    let tls_connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(accept_invalid)
        .build()
        .map_err(|e| e.to_string())?;
    let connector = tokio_tungstenite::Connector::NativeTls(tls_connector);

    // Connect synchronously so the frontend knows WS is up when invoke('connect') returns.
    let (ws_stream, _) = connect_async_tls_with_config(&ws_url, None, false, Some(connector))
        .await
        .map_err(|e| e.to_string())?;

    let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Flush unACKed frames to the new connection before it goes live.
    {
        let state = app.state::<AppState>();
        let queue = state.outgoing_queue.lock().unwrap();
        for (_, frame) in queue.iter() {
            let _ = out_tx.send(frame.clone());
        }
    }

    // Flush pending read receipts from any previous disconnect.
    {
        let state = app.state::<AppState>();
        let pending: Vec<String> =
            state.pending_receipts.lock().unwrap().drain().collect();
        for peer_id in pending {
            let frame = client::build_routing_frame(&peer_id, client::READ_MAGIC, 0);
            let _ = out_tx.send(frame);
        }
    }

    *app.state::<AppState>().ws_tx.lock().unwrap() = Some(out_tx);

    tokio::spawn(client::ws_loop(ws_stream, app.clone(), out_rx));

    // Check SPK age in background — doesn't block WS startup.
    tokio::spawn(rotate_spk_if_needed(app.clone()));

    // Replenish OPK pool if running low (fire-and-forget).
    tokio::spawn(replenish_opks_if_needed(app.clone()));

    Ok(())
}

// ── prepare_session ───────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct PeerInfo {
    pub user_id: String,
    pub username: String,
}

/// If input is a UUID keep it (username = first 8 chars); otherwise resolve by username.
async fn resolve_peer(input: &str, state: &AppState) -> Result<PeerInfo, String> {
    let trimmed = input.trim_start_matches('@');
    if trimmed.len() == 36 && trimmed.chars().filter(|c| *c == '-').count() == 4 {
        return Ok(PeerInfo {
            user_id: trimmed.to_string(),
            username: format!("{}…", &trimmed[..8]),
        });
    }
    #[derive(serde::Deserialize)]
    struct Resp { user_id: String, username: String }
    let resp: Resp = state
        .http
        .get(format!("{}/api/v1/users/by-username/{trimmed}", state.server_url))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|_| format!("user '{trimmed}' not found"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(PeerInfo { user_id: resp.user_id, username: resp.username })
}

// ── Key Transparency ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct KtEntry {
    id: i64,
    key_type: String,
    key_hash: String,
    prev_hash: Option<String>,
    entry_hash: String,
    created_at: i64,
}

fn kt_hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd hex string length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn kt_hex_encode(b: &[u8]) -> String {
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Fetch and verify the key transparency log for `peer_id`.
/// Returns `Err` if the log is broken or if the server's logged key doesn't match `bundle`.
/// Returns `Ok(())` if the log is empty (no entries yet — backwards-compat with pre-KT servers).
async fn verify_key_transparency(
    peer_id: &str,
    bundle: &PreKeyBundle,
    state: &AppState,
) -> Result<(), String> {
    let (last_id, last_hash) = {
        let db = state.db.lock().unwrap();
        db.as_ref()
            .and_then(|c| db::get_key_log_state(c, peer_id))
            .unwrap_or((0, Vec::new()))
    };

    let url = format!(
        "{}/api/v1/users/{}/key-log?since_id={}",
        state.server_url, peer_id, last_id
    );
    let resp = state
        .http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("KT fetch failed: {e}"))?;
    // 404 means the server doesn't have the key-log endpoint yet — treat as no entries.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let entries: Vec<KtEntry> = resp
        .error_for_status()
        .map_err(|e| format!("KT fetch error: {e}"))?
        .json()
        .await
        .map_err(|e| format!("KT parse failed: {e}"))?;

    if entries.is_empty() {
        return Ok(());
    }

    // Verify chain anchor
    let first = &entries[0];
    if last_id == 0 {
        if first.prev_hash.is_some() {
            return Err(format!("KT: first entry for {peer_id} must have prev_hash=null"));
        }
    } else {
        let expected = kt_hex_encode(&last_hash);
        if first.prev_hash.as_deref() != Some(expected.as_str()) {
            return Err(format!(
                "KT: chain anchor broken for {peer_id} at entry {}",
                first.id
            ));
        }
    }

    // Verify internal chain links
    for i in 1..entries.len() {
        if entries[i].prev_hash.as_deref() != Some(entries[i - 1].entry_hash.as_str()) {
            return Err(format!(
                "KT: chain link broken for {peer_id} at entry {}",
                entries[i].id
            ));
        }
    }

    // Verify each entry_hash against the hash-chain formula
    let user_uuid = uuid::Uuid::parse_str(peer_id)
        .map_err(|_| format!("KT: cannot parse peer UUID: {peer_id}"))?;
    let uid_bytes = user_uuid.as_bytes().to_vec();

    for e in &entries {
        let prev_bytes = e
            .prev_hash
            .as_deref()
            .map(|s| kt_hex_decode(s).unwrap_or_else(|_| vec![0u8; 32]))
            .unwrap_or_else(|| vec![0u8; 32]);
        let key_hash_bytes = kt_hex_decode(&e.key_hash)
            .map_err(|err| format!("KT: bad key_hash at entry {}: {err}", e.id))?;

        let mut buf: Vec<u8> = Vec::with_capacity(32 + 16 + 4 + e.key_type.len() + 32 + 8);
        buf.extend_from_slice(&prev_bytes);
        buf.extend_from_slice(&uid_bytes);
        buf.extend_from_slice(&(e.key_type.len() as u32).to_le_bytes());
        buf.extend_from_slice(e.key_type.as_bytes());
        buf.extend_from_slice(&key_hash_bytes);
        buf.extend_from_slice(&e.created_at.to_le_bytes());
        let computed = kt_hex_encode(&Sha256::digest(&buf));

        if computed != e.entry_hash {
            return Err(format!("KT: entry_hash mismatch at entry {} for {peer_id}", e.id));
        }
    }

    // Compare the newest static_bundle log entry against the bundle we received
    if let Some(last_static) = entries.iter().rev().find(|e| e.key_type == "static_bundle") {
        let mut canonical: Vec<u8> = Vec::with_capacity(32 + 32 + 1184);
        canonical.extend_from_slice(bundle.identity_key.as_bytes());
        canonical.extend_from_slice(bundle.signed_prekey.as_bytes());
        if let Some(ref pq) = bundle.pq_spk_public {
            canonical.extend_from_slice(pq);
        }
        let expected_hash = kt_hex_encode(&Sha256::digest(&canonical));
        if last_static.key_hash != expected_hash {
            return Err(format!(
                "KT: key mismatch for {peer_id} — possible MITM key substitution"
            ));
        }
    }

    // Persist verified cursor
    let last = entries.last().unwrap();
    let last_entry_bytes = kt_hex_decode(&last.entry_hash).unwrap_or_else(|_| vec![0u8; 32]);
    {
        let db = state.db.lock().unwrap();
        if let Some(conn) = db.as_ref() {
            db::set_key_log_state(conn, peer_id, last.id, &last_entry_bytes);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn prepare_session(peer_id: String, app: AppHandle) -> Result<PeerInfo, String> {
    let state = app.state::<AppState>();

    let peer = resolve_peer(&peer_id, &state).await?;

    // Fetch peer's prekey bundle
    let bundle_bytes = state
        .http
        .get(format!("{}/api/v1/users/{}/prekeys", state.server_url, peer.user_id))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let bundle: PreKeyBundle =
        keys::bundle_from_bytes(&bundle_bytes).map_err(|e| e.to_string())?;

    // Verify key transparency before proceeding with X3DH.
    // A mismatch means the server substituted a different key — abort the session.
    if let Err(kt_err) = verify_key_transparency(&peer.user_id, &bundle, &state).await {
        app.emit("kt_warning", serde_json::json!({
            "userId": peer.user_id,
            "reason": kt_err,
        }))
        .ok();
        return Err(format!("Key transparency check failed: {kt_err}"));
    }

    // Reconstruct keypair from stored bytes (sync, fast)
    let signing_key_bytes = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.signing_key_bytes
    };

    let keypair = IdentityKeyPair { signing: SigningKey::from_bytes(&signing_key_bytes) };
    let bob_spk_pub = bundle.signed_prekey;
    let (secret, x3dh_header, pq_ct) = x3dh_send(&keypair, &bundle).map_err(|e| e.to_string())?;
    let ratchet = RatchetState::init_alice(&secret, bob_spk_pub);

    {
        let mut sessions = state.sessions.lock().unwrap();
        // Don't overwrite an existing session — if we already received their
        // InitEnvelope and established as Bob, keep that session intact.
        if sessions.contains_key(&peer.user_id) {
            return Ok(peer);
        }
        sessions.insert(peer.user_id.clone(), PeerSession::AlicePending { x3dh_header, pq_ct, ratchet });
    }

    // Store peer's X25519 identity key for safety number verification (Alice side).
    state
        .peer_identity_keys
        .lock()
        .unwrap()
        .insert(peer.user_id.clone(), bundle.identity_key.to_bytes());

    // Cache username so ws_loop can show it in system notifications.
    state
        .peer_names
        .lock()
        .unwrap()
        .insert(peer.user_id.clone(), peer.username.clone());

    Ok(peer)
}

// ── send_message ──────────────────────────────────────────────────────────────

/// Returns the `msg_id` so the frontend can track delivery status (✓ → ✓✓).
#[tauri::command]
pub async fn send_message(
    peer_id: String,
    text: String,
    reply_to: Option<ReplyInfo>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let state = app.state::<AppState>();

    // Assign a unique ID for this message (reset each session; no persistence needed).
    let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);

    // Read session key (fast, no await)
    let session_key = state.session_key.lock().unwrap().clone();

    // Compute timestamp before encoding so payload and DB row share the same ts.
    let ts = store::now_ms();
    let payload = encode_payload(&text, ts, reply_to.as_ref());

    // Build wire frame and snapshot sessions — both under the sessions lock,
    // sync crypto only, no .await inside.
    let (routing_frame, encrypted_sessions) = {
        let mut sessions = state.sessions.lock().unwrap();
        let session = sessions.remove(&peer_id).ok_or("no session — call prepare_session first")?;

        let routing_frame = match session {
            PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                let msg = ratchet.encrypt(payload.as_bytes(), AD);
                let wf = wire::encode(&InitEnvelope {
                    x3dh: x3dh_header,
                    pq_ct,
                    message: msg,
                    ad: AD.to_vec(),
                })
                .map_err(|e| e.to_string())?;
                sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                client::build_routing_frame(&peer_id, &wf, msg_id)
            }
            PeerSession::Established { mut ratchet } => {
                let msg = ratchet.encrypt(payload.as_bytes(), AD);
                let wf = wire::encode(&MessageEnvelope {
                    session_id: [0u8; 16],
                    message: msg,
                    ad: AD.to_vec(),
                })
                .map_err(|e| e.to_string())?;
                sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                client::build_routing_frame(&peer_id, &wf, msg_id)
            }
        };

        let encrypted = match session_key {
            Some(ref key) => store::extract_and_encrypt(&sessions, key),
            None => Vec::new(),
        };
        (routing_frame, encrypted)
    };

    // Pre-extract reply fields for DB storage (store display text, not the JSON payload).
    let (r_ts, r_from, r_text) = match &reply_to {
        None => (None, None, None),
        Some(r) => {
            let quoted: String = if r.text.chars().count() > 100 {
                r.text.chars().take(100).collect::<String>() + "..."
            } else {
                r.text.clone()
            };
            (Some(r.ts), Some(r.from.clone()), Some(quoted))
        }
    };

    // Write to DB before touching the WS — ensures the row exists for ACK updates
    // even if the connection drops between here and the send.
    if let Some(ref key) = session_key {
        let (nonce, ct) = store::encrypt_content(&text, key);
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            let _ = db::insert_sent(
                conn, &peer_id, ts, &nonce, &ct, msg_id,
                r_ts, r_from.as_deref(), r_text.as_deref(),
                None, None, None, None,
                Some(text.as_str()), None,
            );
        }
    }

    // Queue for retry — must be in queue before the live send attempt so that
    // a disconnect between push and send doesn't lose the message.
    state.outgoing_queue.lock().unwrap().push_back((msg_id, routing_frame.clone()));

    // Best-effort live send; if disconnected the queue handles retry on reconnect.
    if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
        let _ = tx.send(routing_frame);
    }

    // Persist sessions asynchronously (fire-and-forget)
    if !encrypted_sessions.is_empty() {
        if let Ok(dir) = store::data_dir(&app) {
            tokio::spawn(store::save_sessions(dir, encrypted_sessions));
        }
    }

    Ok(serde_json::json!({ "id": msg_id, "ts": ts }))
}

// ── get_messages ─────────────────────────────────────────────────────────────

/// Load and decrypt paginated message history for a peer from the local SQLite DB.
///
/// Returns at most `limit` messages. Pass `before_id = None` to start from the newest.
/// Use `db_id` of the oldest message in the result as `before_id` for the next page.
#[tauri::command]
pub fn get_messages(
    peer_id: String,
    limit: u32,
    before_id: Option<i64>,
    app: AppHandle,
) -> Result<Vec<MessageView>, String> {
    let state = app.state::<AppState>();

    let user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone();
    let key = session_key.ok_or("not unlocked")?;

    let rows = {
        let db = state.db.lock().unwrap();
        let conn = db.as_ref().ok_or("db not open")?;
        db::load_for_peer(conn, &peer_id, limit, before_id)?
    };

    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let nonce: [u8; 12] = row.nonce.try_into().map_err(|_| "invalid nonce in db")?;
        // For group received messages use the stored sender_id; for DMs use peer_id.
        let from = if row.direction == "sent" {
            user_id.clone()
        } else if let Some(ref sid) = row.sender_id {
            sid.clone()
        } else {
            peer_id.clone()
        };
        match row.file_id {
            Some(ref fid) => {
                // File message: ct decrypts to JSON-encoded key bytes ([1,2,...,32]).
                match store::decrypt_content(&nonce, &row.ct, &key) {
                    Ok(key_json) => {
                        let file_key: Vec<u8> =
                            serde_json::from_str(&key_json).unwrap_or_default();
                        messages.push(MessageView {
                            db_id: row.db_id,
                            from,
                            text: String::new(),
                            ts: row.ts,
                            id: row.smid.map(|v| v as u32),
                            status: row.status,
                            reply_to_ts: None,
                            reply_to_from: None,
                            reply_to_text: None,
                            file_id: Some(fid.clone()),
                            file_key: Some(file_key),
                            file_name: row.file_name,
                            file_mime: row.file_mime,
                            file_size: row.file_size,
                            thumb_data: row.thumb_data,
                            group_id: row.group_id,
                            sender_id: row.sender_id,
                        });
                    }
                    Err(e) => tracing::warn!("skipping undecryptable file message: {e}"),
                }
            }
            None => {
                match store::decrypt_content(&nonce, &row.ct, &key) {
                    Ok(text) => {
                        messages.push(MessageView {
                            db_id: row.db_id,
                            from,
                            text,
                            ts: row.ts,
                            id: row.smid.map(|v| v as u32),
                            status: row.status,
                            reply_to_ts: row.reply_to_ts,
                            reply_to_from: row.reply_to_from,
                            reply_to_text: row.reply_to_text,
                            file_id: None,
                            file_key: None,
                            file_name: None,
                            file_mime: None,
                            file_size: None,
                            thumb_data: None,
                            group_id: row.group_id,
                            sender_id: row.sender_id,
                        });
                    }
                    Err(e) => tracing::warn!("skipping undecryptable stored message: {e}"),
                }
            }
        }
    }
    Ok(messages)
}

// ── send_file ─────────────────────────────────────────────────────────────────

/// Encrypt a file client-side, upload the ciphertext to the server, then send
/// the file key + metadata to `peer_id` via the Double Ratchet.
///
/// Returns the `msg_id` for delivery tracking (same as `send_message`).
/// The server stores an opaque encrypted blob; it never sees the plaintext.
///
/// > MVP note: uses a single-shot 20 MB POST. For production, implement
/// > chunked / resumable upload on the server side.
#[tauri::command]
pub async fn send_file(
    peer_id: String,
    file_bytes: Vec<u8>,
    file_name: String,
    mime_type: String,
    thumb_bytes: Option<Vec<u8>>,
    app: AppHandle,
) -> Result<FileSentResult, String> {
    const MAX_FILE: usize = 20 * 1024 * 1024;
    if file_bytes.is_empty() {
        return Err("file is empty".into());
    }
    if file_bytes.len() > MAX_FILE {
        return Err("file too large (max 20 MB)".into());
    }

    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    // Generate random ChaCha20Poly1305 key + nonce; prepend nonce to ciphertext.
    let mut file_key = [0u8; 32];
    let mut file_nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut file_key);
    rand::thread_rng().fill_bytes(&mut file_nonce);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&file_key));
    let ct_bytes = cipher
        .encrypt(Nonce::from_slice(&file_nonce), file_bytes.as_slice())
        .map_err(|_| "file encryption failed")?;

    let mut encrypted_blob = Vec::with_capacity(12 + ct_bytes.len());
    encrypted_blob.extend_from_slice(&file_nonce);
    encrypted_blob.extend_from_slice(&ct_bytes);

    // Upload encrypted blob to server.
    #[derive(serde::Deserialize)]
    struct UploadResp { file_id: String }
    let file_id: String = state
        .http
        .post(format!("{}/api/v1/files", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/octet-stream")
        .body(encrypted_blob)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<UploadResp>()
        .await
        .map_err(|e| e.to_string())?
        .file_id;

    let file_size = file_bytes.len() as i64;
    let file_key_vec: Vec<u8> = file_key.to_vec();

    // Encrypt thumbnail with same file_key but a SEPARATE nonce (nonce reuse = catastrophic).
    let (thumb_nonce_vec, thumb_ct_vec, thumb_plain) =
        if let Some(ref tb) = thumb_bytes {
            if mime_type.starts_with("image/") && !tb.is_empty() {
                let mut tnonce = [0u8; 12];
                rand::thread_rng().fill_bytes(&mut tnonce);
                let tct = ChaCha20Poly1305::new(Key::from_slice(&file_key))
                    .encrypt(Nonce::from_slice(&tnonce), tb.as_slice())
                    .unwrap_or_default();
                (Some(tnonce.to_vec()), Some(tct), Some(tb.clone()))
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

    // Build JSON v1 payload — key as array of numbers (no base64 dep needed).
    let mut payload_map = serde_json::json!({
        "v": 1,
        "file_id": file_id,
        "key": file_key_vec,
        "name": file_name,
        "mime": mime_type,
        "size": file_size,
    });
    if let (Some(ref tn), Some(ref tc)) = (&thumb_nonce_vec, &thumb_ct_vec) {
        payload_map["thn"] = serde_json::json!(tn);
        payload_map["thc"] = serde_json::json!(tc);
    }
    let payload = payload_map.to_string();

    let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);
    let session_key = state.session_key.lock().unwrap().clone();

    let (routing_frame, encrypted_sessions) = {
        let mut sessions = state.sessions.lock().unwrap();
        let session = sessions
            .remove(&peer_id)
            .ok_or("no session — call prepare_session first")?;

        let routing_frame = match session {
            PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                let msg = ratchet.encrypt(payload.as_bytes(), AD);
                let wf = wire::encode(&InitEnvelope {
                    x3dh: x3dh_header,
                    pq_ct,
                    message: msg,
                    ad: AD.to_vec(),
                })
                .map_err(|e| e.to_string())?;
                sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                client::build_routing_frame(&peer_id, &wf, msg_id)
            }
            PeerSession::Established { mut ratchet } => {
                let msg = ratchet.encrypt(payload.as_bytes(), AD);
                let wf = wire::encode(&MessageEnvelope {
                    session_id: [0u8; 16],
                    message: msg,
                    ad: AD.to_vec(),
                })
                .map_err(|e| e.to_string())?;
                sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                client::build_routing_frame(&peer_id, &wf, msg_id)
            }
        };

        let encrypted = match session_key {
            Some(ref key) => store::extract_and_encrypt(&sessions, key),
            None => Vec::new(),
        };
        (routing_frame, encrypted)
    };

    // Store the file key encrypted in ct; file metadata in plaintext columns.
    if let Some(ref sk) = session_key {
        let key_json = serde_json::to_string(&file_key_vec).unwrap();
        let (db_nonce, db_ct) = store::encrypt_content(&key_json, sk);
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            let _ = db::insert_sent(
                conn,
                &peer_id,
                store::now_ms(),
                &db_nonce,
                &db_ct,
                msg_id,
                None,
                None,
                None,
                Some(file_id.as_str()),
                Some(file_name.as_str()),
                Some(mime_type.as_str()),
                Some(file_size),
                Some(file_name.as_str()),
                thumb_plain.as_deref(),
            );
        }
    }

    state
        .outgoing_queue
        .lock()
        .unwrap()
        .push_back((msg_id, routing_frame.clone()));
    if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
        let _ = tx.send(routing_frame);
    }

    if !encrypted_sessions.is_empty() {
        if let Ok(dir) = store::data_dir(&app) {
            tokio::spawn(store::save_sessions(dir, encrypted_sessions));
        }
    }

    Ok(FileSentResult { id: msg_id, file_id, file_key: file_key_vec })
}

// ── download_file ─────────────────────────────────────────────────────────────

/// Download and decrypt an E2EE file blob from the server.
///
/// `key_bytes` must be the 32-byte ChaCha20Poly1305 key that was transmitted in
/// the file message. Returns the raw plaintext file bytes.
#[tauri::command]
pub async fn download_file(
    file_id: String,
    key_bytes: Vec<u8>,
    app: AppHandle,
) -> Result<Vec<u8>, String> {
    if key_bytes.len() != 32 {
        return Err(format!("invalid key length: {} (expected 32)", key_bytes.len()));
    }

    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    let blob: Vec<u8> = state
        .http
        .get(format!("{}/api/v1/files/{file_id}", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?
        .to_vec();

    if blob.len() < 12 {
        return Err("file data too short".into());
    }

    let nonce: [u8; 12] = blob[..12].try_into().unwrap();
    let key: [u8; 32] = key_bytes.try_into().unwrap();
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    cipher
        .decrypt(Nonce::from_slice(&nonce), &blob[12..])
        .map_err(|_| "file decryption failed — wrong key or corrupted data".to_string())
}

// ── get_safety_number ─────────────────────────────────────────────────────────

/// Compute a 30-digit safety number for a peer conversation.
///
/// The number is derived from both parties' X25519 identity keys (canonical
/// ordering so both sides see the same digits), formatted as 6 groups of 5
/// decimal digits, e.g. `"12345 67890 11111 22222 33333 44444"`.
#[tauri::command]
pub fn get_safety_number(peer_id: String, app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();

    let signing_key_bytes = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.signing_key_bytes
    };

    let our_key = IdentityKeyPair { signing: SigningKey::from_bytes(&signing_key_bytes) }
        .dh_public()
        .to_bytes();

    let peer_key = state
        .peer_identity_keys
        .lock()
        .unwrap()
        .get(&peer_id)
        .copied()
        .ok_or("no identity key for this peer yet — exchange at least one message first")?;

    // Canonical ordering: sort keys so both sides compute the same hash.
    let (first, second) = if our_key <= peer_key { (our_key, peer_key) } else { (peer_key, our_key) };

    let hash = Sha256::new().chain_update(first).chain_update(second).finalize();

    // Format first 30 bytes as 6 groups of 5 decimal digits.
    // Each group: 5 bytes interpreted as big-endian u40, then mod 100_000.
    let number = (0..6)
        .map(|i| {
            let off = i * 5;
            let v = u64::from_be_bytes([
                0, 0, 0,
                hash[off], hash[off + 1], hash[off + 2], hash[off + 3], hash[off + 4],
            ]) % 100_000;
            format!("{v:05}")
        })
        .collect::<Vec<_>>()
        .join(" ");

    Ok(number)
}

// ── send_typing ───────────────────────────────────────────────────────────────

/// Send a typing indicator to a peer. The recipient's client shows a "typing…"
/// indicator that auto-expires after a few seconds if no further events arrive.
#[tauri::command]
pub async fn send_typing(peer_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let frame = client::build_routing_frame(&peer_id, client::TYPING_MAGIC, 0);
    state
        .ws_tx
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("not connected")?
        .send(frame)
        .map_err(|_| "WS send channel closed".to_string())?;
    Ok(())
}

// ── send_read_receipt ─────────────────────────────────────────────────────────

/// Tell `peer_id` that we've read all their messages. Triggers blue ✓✓ on their side.
/// If the WS is currently disconnected, queues the receipt for delivery on next connect.
#[tauri::command]
pub async fn send_read_receipt(peer_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // Verify identity exists before queuing (avoid receipts for logged-out state).
    {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?;
    }
    let frame = client::build_routing_frame(&peer_id, client::READ_MAGIC, 0);
    let sent = state
        .ws_tx
        .lock()
        .unwrap()
        .as_ref()
        .map(|tx| tx.send(frame).is_ok())
        .unwrap_or(false);
    if !sent {
        state.pending_receipts.lock().unwrap().insert(peer_id);
    }
    Ok(())
}

// ── send_group_read_receipt ───────────────────────────────────────────────────

/// Broadcast our read watermark to all group members.
/// The frame is routed by the server the same way as a group message (fan-out by gid).
#[tauri::command]
pub async fn send_group_read_receipt(group_id: String, ts: i64, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?;
    }
    let frame = client::build_group_read_frame(&group_id, ts);
    let _sent = state
        .ws_tx
        .lock()
        .unwrap()
        .as_ref()
        .map(|tx| tx.send(frame).is_ok())
        .unwrap_or(false);
    Ok(())
}

// ── get_group_read_marks ──────────────────────────────────────────────────────

/// Return per-member read watermarks for a group: `{ user_id: ts_ms }`.
#[tauri::command]
pub fn get_group_read_marks(
    group_id: String,
    app: AppHandle,
) -> std::collections::HashMap<String, i64> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    match *db {
        Some(ref conn) => crate::db::get_group_read_marks(conn, &group_id),
        None => std::collections::HashMap::new(),
    }
}

// ── export_identity ───────────────────────────────────────────────────────────

/// Export the identity to an encrypted backup blob.
///
/// Format: `b"MBAK" [16B Argon2id salt] [12B nonce] [ChaCha20Poly1305(identity_json)]`
///
/// Sessions are NOT exported — they can be rebuilt by exchanging messages after restore.
#[tauri::command]
pub async fn export_identity(password: String, app: AppHandle) -> Result<Vec<u8>, String> {
    let dir = store::data_dir(&app)?;
    let identity_bytes = tokio::fs::read(dir.join("identity.json"))
        .await
        .map_err(|_| "identity not found — register first")?;

    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let salt_copy = salt;
    let key = tokio::task::spawn_blocking(move || store::derive_session_key(&password, &salt_copy))
        .await
        .map_err(|e| e.to_string())?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), identity_bytes.as_slice())
        .map_err(|_| "encryption failed")?;

    let mut blob = Vec::with_capacity(4 + 16 + 12 + ciphertext.len());
    blob.extend_from_slice(b"MBAK");
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

// ── import_identity ───────────────────────────────────────────────────────────

/// Restore identity from a backup blob produced by `export_identity`.
///
/// Clears all local sessions and messages, restores the identity, and repopulates
/// AppState. The caller must then invoke `unlock` with their session password.
#[tauri::command]
pub async fn import_identity(
    password: String,
    data: Vec<u8>,
    app: AppHandle,
) -> Result<UserInfo, String> {
    if data.len() < 4 + 16 + 12 + 16 {
        return Err("backup file is too short".into());
    }
    if &data[..4] != b"MBAK" {
        return Err("not a valid messenger backup file".into());
    }

    let salt: [u8; 16] = data[4..20].try_into().unwrap();
    let nonce: [u8; 12] = data[20..32].try_into().unwrap();
    let ciphertext = data[32..].to_vec();

    let key = tokio::task::spawn_blocking(move || store::derive_session_key(&password, &salt))
        .await
        .map_err(|e| e.to_string())?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_slice())
        .map_err(|_| "incorrect_password")?;

    let stored: store::StoredIdentity =
        serde_json::from_slice(&plaintext).map_err(|e| format!("invalid backup content: {e}"))?;

    let dir = store::data_dir(&app)?;
    store::save(&dir.join("identity.json"), &stored)
        .await
        .map_err(|e| e.to_string())?;

    // Clear sessions and messages — they are per-device and not in the backup.
    tokio::fs::remove_file(dir.join("sessions.bin")).await.ok();
    tokio::fs::remove_file(dir.join("sessions.bin.tmp")).await.ok();
    tokio::fs::remove_file(dir.join("sessions.salt")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db-wal")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db-shm")).await.ok();

    // Reset AppState so a subsequent unlock starts from scratch.
    let state = app.state::<AppState>();
    *state.ws_tx.lock().unwrap() = None;
    *state.session_key.lock().unwrap() = None;
    *state.db.lock().unwrap() = None;
    state.sessions.lock().unwrap().clear();
    state.outgoing_queue.lock().unwrap().clear();
    state.pending_receipts.lock().unwrap().clear();
    state.peer_names.lock().unwrap().clear();
    state.peer_identity_keys.lock().unwrap().clear();

    populate_app_state(&app, &stored);

    Ok(UserInfo { user_id: stored.user_id, username: stored.username })
}

// ── clear_identity ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn clear_identity(app: AppHandle) -> Result<(), String> {
    let dir = store::data_dir(&app)?;
    tokio::fs::remove_file(dir.join("identity.json")).await.ok();
    tokio::fs::remove_file(dir.join("sessions.bin")).await.ok();
    tokio::fs::remove_file(dir.join("sessions.bin.tmp")).await.ok();
    tokio::fs::remove_file(dir.join("sessions.salt")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db-wal")).await.ok();
    tokio::fs::remove_file(dir.join("messages.db-shm")).await.ok();
    let state = app.state::<AppState>();
    *state.identity.lock().unwrap() = None;
    *state.ws_tx.lock().unwrap() = None;
    *state.session_key.lock().unwrap() = None;
    *state.db.lock().unwrap() = None;
    state.sessions.lock().unwrap().clear();
    state.outgoing_queue.lock().unwrap().clear();
    state.pending_receipts.lock().unwrap().clear();
    state.peer_names.lock().unwrap().clear();
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn populate_app_state(app: &AppHandle, stored: &StoredIdentity) {
    *app.state::<AppState>().identity.lock().unwrap() = Some(IdentityState {
        user_id: stored.user_id.clone(),
        username: stored.username.clone(),
        token: stored.token.clone(),
        signing_key_bytes: stored.signing_key,
        spk_secret_bytes: stored.spk_secret,
        spk_id: stored.spk_id,
        spk_rotation_ts: stored.spk_rotation_ts,
        opk_secret_bytes: stored.opk_secrets.clone(),
        opk_next_id: stored.opk_next_id,
        pq_spk_secret_bytes: stored.pq_spk_secret.clone(),
    });
}

// ── SPK rotation ──────────────────────────────────────────────────────────────

const SPK_ROTATION_INTERVAL_MS: i64 = 7 * 24 * 3600 * 1000; // 7 days

/// Generate a new SPK, upload a fresh prekey bundle, update identity on disk and in memory.
/// No-ops if last rotation was < 7 days ago. Designed to be fire-and-forget after connect.
async fn rotate_spk_if_needed(app: AppHandle) {
    let (user_id, token, signing_key_bytes, spk_id, spk_rotation_ts) = {
        let state = app.state::<AppState>();
        let guard = state.identity.lock().unwrap();
        let Some(id) = guard.as_ref() else { return };
        (
            id.user_id.clone(),
            id.token.clone(),
            id.signing_key_bytes,
            id.spk_id,
            id.spk_rotation_ts,
        )
    };

    if store::now_ms() - spk_rotation_ts < SPK_ROTATION_INTERVAL_MS {
        return;
    }

    let new_spk_id = spk_id.wrapping_add(1);
    let keypair = IdentityKeyPair { signing: SigningKey::from_bytes(&signing_key_bytes) };
    let new_spk = messenger_crypto::keys::SignedPreKey::generate(new_spk_id);
    let spk_secret_bytes: [u8; 32] = new_spk.secret.to_bytes();

    // Also regenerate the ML-KEM-768 PQ SPK on each rotation.
    let (pq_ek_bytes, pq_dk_bytes) = messenger_crypto::keys::generate_pq_spk();
    let pq_spk_sig: Vec<u8> = keypair.sign(&pq_ek_bytes).to_vec();

    let bundle = PreKeyBundle {
        identity_key: keypair.dh_public(),
        identity_key_ed: keypair.public().verifying,
        signed_prekey: new_spk.public(),
        signed_prekey_id: new_spk_id,
        signed_prekey_sig: new_spk.sign(&keypair),
        one_time_prekey: None,
        one_time_prekey_id: None,
        pq_spk_public: Some(pq_ek_bytes),
        pq_spk_sig: Some(pq_spk_sig),
    };
    let bundle_bytes = match postcard::to_stdvec(&bundle) {
        Ok(b) => b,
        Err(e) => { tracing::error!("SPK rotation encode: {e}"); return; }
    };

    let state = app.state::<AppState>();
    let result = state
        .http
        .put(format!("{}/api/v1/users/{user_id}/prekeys", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/octet-stream")
        .body(bundle_bytes)
        .send()
        .await
        .and_then(|r| r.error_for_status());

    match result {
        Ok(_) => {
            let rotation_ts = store::now_ms();
            // Update in-memory state.
            {
                let mut guard = state.identity.lock().unwrap();
                if let Some(ref mut id) = *guard {
                    id.spk_id = new_spk_id;
                    id.spk_secret_bytes = spk_secret_bytes;
                    id.spk_rotation_ts = rotation_ts;
                    id.pq_spk_secret_bytes = pq_dk_bytes.clone();
                }
            }
            // Persist to disk: read → patch → write.
            let Ok(dir) = store::data_dir(&app) else { return };
            let path = dir.join("identity.json");
            let Ok(bytes) = tokio::fs::read(&path).await else { return };
            let Ok(mut stored) = serde_json::from_slice::<store::StoredIdentity>(&bytes) else { return };
            stored.spk_id = new_spk_id;
            stored.spk_secret = spk_secret_bytes;
            stored.spk_rotation_ts = rotation_ts;
            stored.pq_spk_secret = pq_dk_bytes;
            if let Err(e) = store::save(&path, &stored).await {
                tracing::error!("SPK rotation save: {e}");
            } else {
                tracing::info!(new_spk_id, "SPK rotated");
            }
        }
        Err(e) => tracing::error!("SPK rotation upload failed: {e}"),
    }
}

// ── OPK replenishment ─────────────────────────────────────────────────────────

const OPK_BATCH_SIZE: u32 = 10;
const OPK_LOW_WATER: usize = 3;

/// Generate a new batch of OPKs and upload them when the local pool runs low.
/// Designed to be fire-and-forget after connect (same pattern as SPK rotation).
async fn replenish_opks_if_needed(app: AppHandle) {
    let (user_id, token, opk_count, opk_next_id) = {
        let state = app.state::<AppState>();
        let guard = state.identity.lock().unwrap();
        let Some(id) = guard.as_ref() else { return };
        (id.user_id.clone(), id.token.clone(), id.opk_secret_bytes.len(), id.opk_next_id)
    };

    if opk_count >= OPK_LOW_WATER {
        return;
    }

    let mut new_secrets: Vec<(u32, [u8; 32])> = Vec::new();
    let mut pub_keys: Vec<(u32, [u8; 32])> = Vec::new();
    for i in 0..OPK_BATCH_SIZE {
        let opk_id = opk_next_id + i;
        let opk = OneTimePreKey::generate(opk_id);
        pub_keys.push((opk_id, opk.public().to_bytes()));
        new_secrets.push((opk_id, opk.secret.to_bytes()));
    }
    let new_next_id = opk_next_id + OPK_BATCH_SIZE;

    let body = match postcard::to_stdvec(&pub_keys) {
        Ok(b) => b,
        Err(e) => { tracing::error!("OPK batch encode: {e}"); return; }
    };

    let state = app.state::<AppState>();
    let result = state
        .http
        .post(format!("{}/api/v1/users/{user_id}/opks", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/octet-stream")
        .body(body)
        .send()
        .await
        .and_then(|r| r.error_for_status());

    match result {
        Ok(_) => {
            {
                let mut guard = state.identity.lock().unwrap();
                if let Some(ref mut id) = *guard {
                    id.opk_secret_bytes.extend_from_slice(&new_secrets);
                    id.opk_next_id = new_next_id;
                }
            }
            let Ok(dir) = store::data_dir(&app) else { return };
            let path = dir.join("identity.json");
            let Ok(bytes) = tokio::fs::read(&path).await else { return };
            let Ok(mut stored) = serde_json::from_slice::<store::StoredIdentity>(&bytes) else { return };
            stored.opk_secrets.extend_from_slice(&new_secrets);
            stored.opk_next_id = new_next_id;
            if let Err(e) = store::save(&path, &stored).await {
                tracing::error!("OPK replenish save: {e}");
            } else {
                tracing::info!(new_next_id, "OPK pool replenished");
            }
        }
        Err(e) => tracing::error!("OPK replenish upload failed: {e}"),
    }
}

// ── Group helpers ─────────────────────────────────────────────────────────────

fn encode_group_payload(text: &str, ts: i64, group_id: &str, reply_to: Option<&ReplyInfo>) -> String {
    let mut obj = serde_json::json!({ "v": 1, "text": text, "ts": ts, "gid": group_id });
    if let Some(r) = reply_to {
        let quoted: String = if r.text.chars().count() > 100 {
            r.text.chars().take(100).collect::<String>() + "..."
        } else {
            r.text.clone()
        };
        obj["r"] = serde_json::json!({ "ts": r.ts, "f": r.from, "t": quoted });
    }
    obj.to_string()
}

// ── create_group ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn create_group(
    name: String,
    member_ids: Vec<String>,
    app: AppHandle,
) -> Result<crate::GroupInfo, String> {
    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    #[derive(serde::Deserialize)]
    struct MemberResp { user_id: String, username: String }
    #[derive(serde::Deserialize)]
    struct GroupResp { group_id: String, name: String, members: Vec<MemberResp> }

    let resp: GroupResp = state
        .http
        .post(format!("{}/api/v1/groups", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({ "name": name, "member_ids": member_ids }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<GroupResp>()
        .await
        .map_err(|e| e.to_string())?;

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };

    // Pre-establish DR sessions with all members so the first message can be sent immediately.
    for m in &resp.members {
        if m.user_id != my_user_id {
            // Ignore errors — sessions will be retried on send.
            prepare_session(m.user_id.clone(), app.clone()).await.ok();
        }
    }

    let group_info = crate::GroupInfo {
        group_id: resp.group_id.clone(),
        name: resp.name,
        members: resp.members.iter().map(|m| crate::MemberInfo {
            user_id: m.user_id.clone(),
            username: m.username.clone(),
        }).collect(),
    };

    state.groups.lock().unwrap().insert(resp.group_id.clone(), group_info.clone());

    // Distribute Sender Key to all members so subsequent sends use the O(1) SK path.
    // Fire-and-forget: group is usable even if distribution fails (falls back to DR fan-out).
    let app2 = app.clone();
    let gid2 = resp.group_id.clone();
    tokio::spawn(async move { distribute_sender_key(gid2, app2).await.ok(); });

    Ok(group_info)
}

// ── load_groups ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn load_groups(app: AppHandle) -> Result<Vec<crate::GroupInfo>, String> {
    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    #[derive(serde::Deserialize)]
    struct MemberResp { user_id: String, username: String }
    #[derive(serde::Deserialize)]
    struct GroupResp { group_id: String, name: String, members: Vec<MemberResp> }

    let raw: Vec<GroupResp> = state
        .http
        .get(format!("{}/api/v1/groups", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<Vec<GroupResp>>()
        .await
        .map_err(|e| e.to_string())?;

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };

    let mut result = Vec::with_capacity(raw.len());
    for g in raw {
        // Cache peer display names for ws_loop notifications.
        for m in &g.members {
            if m.user_id != my_user_id {
                state.peer_names.lock().unwrap().insert(m.user_id.clone(), m.username.clone());
            }
        }
        let gi = crate::GroupInfo {
            group_id: g.group_id.clone(),
            name: g.name,
            members: g.members.iter().map(|m| crate::MemberInfo {
                user_id: m.user_id.clone(),
                username: m.username.clone(),
            }).collect(),
        };
        state.groups.lock().unwrap().insert(g.group_id, gi.clone());
        result.push(gi);
    }

    Ok(result)
}

// ── send_group_message ────────────────────────────────────────────────────────

/// Fan-out: one DR-encrypted frame per member (excluding self).
// ── distribute_sender_key ─────────────────────────────────────────────────────

/// Generate a fresh sender chain key for `group_id` and distribute it to all
/// group members via individual Double Ratchet messages (O(N), one-time setup).
/// After distribution, subsequent sends use the O(1) Sender Keys path.
#[tauri::command]
pub async fn distribute_sender_key(group_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;

    // Generate a fresh 32-byte chain key and store locally.
    let mut chain_key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut chain_key);
    {
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            crate::db::set_sender_chain(conn, &group_id, &my_user_id, &chain_key, 0)?;
        }
    }

    // SKD payload: {"v":1,"skd":{"gid":"<gid>","ck":[...32 bytes...]}}
    let ck_vec: Vec<u8> = chain_key.to_vec();
    let ck_json = serde_json::to_string(&ck_vec).map_err(|e| e.to_string())?;
    let skd_payload = format!(r#"{{"v":1,"skd":{{"gid":"{group_id}","ck":{ck_json}}}}}"#);

    let members: Vec<crate::MemberInfo> = {
        let groups = state.groups.lock().unwrap();
        groups
            .get(&group_id)
            .ok_or("group not found — call load_groups first")?
            .members
            .clone()
    };

    for member in &members {
        if member.user_id == my_user_id {
            continue;
        }
        let peer_id = member.user_id.clone();
        prepare_session(peer_id.clone(), app.clone()).await.ok();

        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);
        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(peer_id.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(skd_payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope {
                        x3dh: x3dh_header, pq_ct, message: msg, ad: AD.to_vec(),
                    }).map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(skd_payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope {
                        session_id: [0u8; 16], message: msg, ad: AD.to_vec(),
                    }).map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
            };
            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };
        state.outgoing_queue.lock().unwrap().push_back((msg_id, routing_frame.clone()));
        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }
        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    // All DR fan-outs succeeded — mark the chain as ready for SK sends.
    {
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            let _ = crate::db::mark_sk_distributed(conn, &group_id, &my_user_id);
        }
    }
    Ok(())
}

// ── send_group_message ────────────────────────────────────────────────────────

/// The decrypted text is stored once in the local DB with `peer_id = group_id`.
#[tauri::command]
pub async fn send_group_message(
    group_id: String,
    text: String,
    reply_to: Option<ReplyInfo>,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;

    let members: Vec<crate::MemberInfo> = {
        let groups = state.groups.lock().unwrap();
        groups
            .get(&group_id)
            .ok_or("group not found — call load_groups first")?
            .members
            .clone()
    };

    let ts = store::now_ms();
    let payload = encode_group_payload(&text, ts, &group_id, reply_to.as_ref());
    let (r_ts, r_from, r_text) = match &reply_to {
        Some(r) => (Some(r.ts), Some(r.from.clone()), Some(r.text.clone())),
        None => (None, None, None),
    };

    // ── Sender Keys fast path (O(1)) ─────────────────────────────────────────
    // Only use SK if chain exists AND distribution was confirmed (distributed=1).
    // This prevents sending SK messages when members haven't received the chain key yet.
    let sk_chain = {
        let db = state.db.lock().unwrap();
        db.as_ref().and_then(|c| crate::db::get_sender_chain(c, &group_id, &my_user_id))
            .filter(|(_, _, distributed)| *distributed)
    };
    if let Some((chain_key, _, _)) = sk_chain {
        let counter = {
            let db = state.db.lock().unwrap();
            db.as_ref()
                .and_then(|c| crate::db::next_send_counter(c, &group_id, &my_user_id))
                .ok_or("failed to get send counter")?
        };
        let sk_payload = messenger_crypto::sender_keys::encrypt(&chain_key, counter, payload.as_bytes());
        let frame = client::build_group_broadcast_frame(&group_id, &sk_payload);
        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(frame);
        }
        // Store sent message locally.
        let (nonce, ct) = store::encrypt_content(&text, &session_key);
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            let _ = crate::db::insert_group_sent(
                conn, &group_id, &my_user_id, ts, &nonce, &ct,
                r_ts, r_from.as_deref(), r_text.as_deref(),
                None, None, None, None, Some(text.as_str()), None,
            );
        }
        return Ok(());
    }
    // ── Fallback: DR fan-out (O(N)) — also distribute SK for future sends ────
    // Distribute in background so the first message still goes through immediately.
    {
        let app2 = app.clone();
        let gid2 = group_id.clone();
        tokio::spawn(async move { distribute_sender_key(gid2, app2).await.ok(); });
    }

    for member in &members {
        if member.user_id == my_user_id {
            continue;
        }
        let peer_id = member.user_id.clone();

        // Ensure a DR session exists — no-op if already established.
        prepare_session(peer_id.clone(), app.clone()).await.ok();

        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);

        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(peer_id.as_str()) {
                Some(s) => s,
                None => {
                    tracing::warn!("no session for group member {peer_id}, skipping");
                    continue;
                }
            };

            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope {
                        x3dh: x3dh_header,
                        pq_ct,
                        message: msg,
                        ad: AD.to_vec(),
                    })
                    .map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope {
                        session_id: [0u8; 16],
                        message: msg,
                        ad: AD.to_vec(),
                    })
                    .map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
            };

            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };

        state.outgoing_queue.lock().unwrap().push_back((msg_id, routing_frame.clone()));
        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }
        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    // Store one "sent" row for the group (peer_id = group_id for group history lookup).
    let (nonce, ct) = store::encrypt_content(&text, &session_key);
    let db = state.db.lock().unwrap();
    if let Some(ref conn) = *db {
        let _ = crate::db::insert_group_sent(
            conn,
            &group_id,
            &my_user_id,
            ts,
            &nonce,
            &ct,
            r_ts,
            r_from.as_deref(),
            r_text.as_deref(),
            None, None, None, None,
            Some(text.as_str()), None,
        );
    }

    Ok(())
}

// ── get_group_messages ────────────────────────────────────────────────────────

/// Load paginated group message history. Group messages are stored with
/// `peer_id = group_id`, so this delegates directly to `get_messages`.
#[tauri::command]
pub fn get_group_messages(
    group_id: String,
    limit: u32,
    before_id: Option<i64>,
    app: AppHandle,
) -> Result<Vec<MessageView>, String> {
    get_messages(group_id, limit, before_id, app)
}

// ── search_messages ───────────────────────────────────────────────────────────

/// Full-text search across all locally stored message plaintext (FTS5).
/// Returns up to `limit` results (default 30) ordered by relevance.
/// Wraps FTS5 errors (malformed query) into an empty result instead of propagating.
#[tauri::command]
pub fn search_messages(
    query: String,
    limit: Option<u32>,
    app: AppHandle,
) -> Result<Vec<db::SearchHit>, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let state = app.state::<AppState>();
    let db_guard = state.db.lock().unwrap();
    let conn = db_guard.as_ref().ok_or("not unlocked")?;
    db::search_messages(conn, &q, limit.unwrap_or(30))
}

// ── send_group_file ───────────────────────────────────────────────────────────

/// Encrypt a file and fan-out to all group members via Double Ratchet.
/// Mirrors send_group_message but carries file metadata in the payload.
#[tauri::command]
pub async fn send_group_file(
    group_id: String,
    file_bytes: Vec<u8>,
    file_name: String,
    mime_type: String,
    thumb_bytes: Option<Vec<u8>>,
    app: AppHandle,
) -> Result<GroupFileSentResult, String> {
    const MAX_FILE: usize = 20 * 1024 * 1024;
    if file_bytes.is_empty() { return Err("file is empty".into()); }
    if file_bytes.len() > MAX_FILE { return Err("file too large (max 20 MB)".into()); }

    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };
    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;

    // Encrypt file client-side.
    let mut file_key = [0u8; 32];
    let mut file_nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut file_key);
    rand::thread_rng().fill_bytes(&mut file_nonce_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&file_key));
    let ct_bytes = cipher
        .encrypt(Nonce::from_slice(&file_nonce_bytes), file_bytes.as_slice())
        .map_err(|_| "file encryption failed")?;
    let mut encrypted_blob = Vec::with_capacity(12 + ct_bytes.len());
    encrypted_blob.extend_from_slice(&file_nonce_bytes);
    encrypted_blob.extend_from_slice(&ct_bytes);

    // Upload to server.
    #[derive(serde::Deserialize)]
    struct UploadResp { file_id: String }
    let file_id: String = state
        .http
        .post(format!("{}/api/v1/files", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/octet-stream")
        .body(encrypted_blob)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json::<UploadResp>()
        .await
        .map_err(|e| e.to_string())?
        .file_id;

    let file_size = file_bytes.len() as i64;
    let file_key_vec: Vec<u8> = file_key.to_vec();

    // Encrypt thumbnail with same file_key but a SEPARATE nonce.
    let (thumb_nonce_vec, thumb_ct_vec, thumb_plain) =
        if let Some(ref tb) = thumb_bytes {
            if mime_type.starts_with("image/") && !tb.is_empty() {
                let mut tnonce = [0u8; 12];
                rand::thread_rng().fill_bytes(&mut tnonce);
                let tct = ChaCha20Poly1305::new(Key::from_slice(&file_key))
                    .encrypt(Nonce::from_slice(&tnonce), tb.as_slice())
                    .unwrap_or_default();
                (Some(tnonce.to_vec()), Some(tct), Some(tb.clone()))
            } else {
                (None, None, None)
            }
        } else {
            (None, None, None)
        };

    // Build JSON v1 payload with gid.
    let mut payload_map = serde_json::json!({
        "v": 1,
        "file_id": file_id,
        "key": file_key_vec,
        "name": file_name,
        "mime": mime_type,
        "size": file_size,
        "gid": group_id,
    });
    if let (Some(ref tn), Some(ref tc)) = (&thumb_nonce_vec, &thumb_ct_vec) {
        payload_map["thn"] = serde_json::json!(tn);
        payload_map["thc"] = serde_json::json!(tc);
    }
    let payload = payload_map.to_string();

    let members: Vec<crate::MemberInfo> = {
        let groups = state.groups.lock().unwrap();
        groups.get(&group_id)
            .ok_or("group not found — call load_groups first")?
            .members.clone()
    };

    let ts = store::now_ms();

    for member in &members {
        if member.user_id == my_user_id { continue; }
        let peer_id = member.user_id.clone();

        prepare_session(peer_id.clone(), app.clone()).await.ok();

        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);

        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(peer_id.as_str()) {
                Some(s) => s,
                None => { tracing::warn!("no session for group member {peer_id}, skipping"); continue; }
            };
            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope { x3dh: x3dh_header, pq_ct, message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope { session_id: [0u8; 16], message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(peer_id.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&peer_id, &wf, msg_id)
                }
            };
            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };

        state.outgoing_queue.lock().unwrap().push_back((msg_id, routing_frame.clone()));
        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }
        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    // Store one sent row with file metadata.
    let key_json = serde_json::to_string(&file_key_vec).unwrap();
    let (db_nonce, db_ct) = store::encrypt_content(&key_json, &session_key);
    let db = state.db.lock().unwrap();
    if let Some(ref conn) = *db {
        let _ = crate::db::insert_group_sent(
            conn, &group_id, &my_user_id, ts,
            &db_nonce, &db_ct,
            None, None, None,
            Some(&file_id), Some(&file_name), Some(&mime_type), Some(file_size),
            Some(file_name.as_str()), thumb_plain.as_deref(),
        );
    }

    Ok(GroupFileSentResult { file_id, file_key: file_key_vec })
}

// ── Unread tracking ───────────────────────────────────────────────────────────

/// Mark all messages in `peer_id` as read up to `ts` (inclusive).
/// Called by the frontend 1 s after the user opens or stays in a conversation.
#[tauri::command]
pub fn mark_as_read(peer_id: String, ts: i64, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db_guard = state.db.lock().unwrap();
    let conn = db_guard.as_ref().ok_or("not unlocked")?;
    db::set_last_read(conn, &peer_id, ts)
}

/// Return the last-read timestamp for a conversation (0 = never read).
#[tauri::command]
pub fn get_last_read_ts(peer_id: String, app: AppHandle) -> i64 {
    let state = app.state::<AppState>();
    let db_guard = state.db.lock().unwrap();
    let conn = match db_guard.as_ref() { Some(c) => c, None => return 0 };
    db::get_last_read(conn, &peer_id)
}

/// Return unread counts keyed by peer_id for all conversations with at least one unread.
#[tauri::command]
pub fn get_unread_counts(app: AppHandle) -> std::collections::HashMap<String, i64> {
    let state = app.state::<AppState>();
    let db_guard = state.db.lock().unwrap();
    let conn = match db_guard.as_ref() { Some(c) => c, None => return Default::default() };
    db::get_all_unread_counts(conn)
}

// ── Reactions ─────────────────────────────────────────────────────────────────

/// Validate, store, and forward a reaction. `add=true` → add, `add=false` → remove.
/// For groups, fans out to all members (same pattern as send_group_message).
#[tauri::command]
pub async fn send_reaction(
    peer_id: String,
    msg_ts: i64,
    msg_from: String,
    emoji: String,
    add: bool,
    app: AppHandle,
) -> Result<(), String> {
    use unicode_segmentation::UnicodeSegmentation;
    if emoji.graphemes(true).count() != 1 {
        return Err("Invalid emoji: must be a single grapheme".into());
    }

    let state = app.state::<AppState>();
    let (my_user_id, session_key) = {
        let guard = state.identity.lock().unwrap();
        let id = guard.as_ref().ok_or("not registered")?;
        let sk = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;
        (id.user_id.clone(), sk)
    };

    // Persist locally first.
    {
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            if add {
                db::add_reaction(conn, &peer_id, msg_ts, &msg_from, &my_user_id, &emoji)?;
            } else {
                db::remove_reaction(conn, &peer_id, msg_ts, &msg_from, &my_user_id)?;
            }
        }
    }

    let payload = serde_json::json!({
        "v": 1,
        "react": { "msg_ts": msg_ts, "msg_from": msg_from, "emoji": emoji, "add": add }
    })
    .to_string();

    // Determine recipients.
    let recipients: Vec<String> = {
        let groups = state.groups.lock().unwrap();
        if let Some(g) = groups.get(&peer_id) {
            g.members.iter().map(|m| m.user_id.clone()).collect()
        } else {
            vec![peer_id.clone()]
        }
    };

    for recipient in recipients {
        if recipient == my_user_id { continue; }
        // Ensure session exists before locking sessions map.
        prepare_session(recipient.clone(), app.clone()).await.ok();
        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);
        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(recipient.as_str()) {
                Some(s) => s,
                None => { tracing::warn!("no session for reaction recipient {recipient}"); continue; }
            };
            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope {
                        x3dh: x3dh_header, pq_ct, message: msg, ad: AD.to_vec(),
                    }).map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&recipient, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope {
                        session_id: [0u8; 16], message: msg, ad: AD.to_vec(),
                    }).map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(&recipient, &wf, msg_id)
                }
            };
            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };
        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }
        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    Ok(())
}

/// Load all reactions for a conversation.
#[tauri::command]
pub fn get_reactions(peer_id: String, app: AppHandle) -> Vec<db::ReactionRow> {
    let state = app.state::<AppState>();
    let db_guard = state.db.lock().unwrap();
    let conn = match db_guard.as_ref() { Some(c) => c, None => return vec![] };
    db::get_reactions(conn, &peer_id)
}

// ── leave_group ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn leave_group(group_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (token, user_id) = {
        let guard = state.identity.lock().unwrap();
        let id = guard.as_ref().ok_or("not registered")?;
        (id.token.clone(), id.user_id.clone())
    };

    state
        .http
        .delete(format!("{}/api/v1/groups/{}/members/{}", state.server_url, group_id, user_id))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    state.groups.lock().unwrap().remove(&group_id);
    Ok(())
}

// ── list_sessions ─────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub device_name: String,
    pub device_id: String,
    pub created_at: String,
    pub last_seen: String,
}

#[tauri::command]
pub async fn list_sessions(app: AppHandle) -> Result<Vec<SessionInfo>, String> {
    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    let sessions: Vec<SessionInfo> = state
        .http
        .get(format!("{}/api/v1/sessions", state.server_url))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    Ok(sessions)
}

// ── revoke_session ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn revoke_session(session_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let token = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.token.clone()
    };

    state
        .http
        .delete(format!("{}/api/v1/sessions/{}", state.server_url, session_id))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    Ok(())
}

// ── get_device_id ─────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_device_id(app: AppHandle) -> Result<String, String> {
    let data_dir = store::data_dir(&app)?;
    Ok(store::get_or_create_device_id(&data_dir))
}

// ── save_draft / get_draft ────────────────────────────────────────────────────

#[tauri::command]
pub fn save_draft(peer_id: String, text: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    if let Some(ref conn) = *db {
        db::set_draft(conn, &peer_id, &text)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_draft(peer_id: String, app: AppHandle) -> String {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    match db.as_ref() {
        Some(conn) => db::get_draft(conn, &peer_id),
        None => String::new(),
    }
}

// ── mute / notification settings ─────────────────────────────────────────────

#[tauri::command]
pub fn set_mute(
    peer_id: String,
    mute_hours: Option<i64>,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let now = store::now_ms();
    let (notifications_enabled, mute_until) = match mute_hours {
        None => (false, 0i64),                          // permanent mute
        Some(0) => (true, 0i64),                        // unmute
        Some(h) => (false, now + h * 3_600_000),        // timed mute
    };
    let db = state.db.lock().unwrap();
    if let Some(ref conn) = *db {
        db::set_mute(conn, &peer_id, notifications_enabled, mute_until)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_mute(peer_id: String, app: AppHandle) -> db::MuteSettings {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    match db.as_ref() {
        Some(conn) => db::get_mute(conn, &peer_id, store::now_ms()),
        None => db::MuteSettings { notifications_enabled: true, mute_until: 0, is_muted: false },
    }
}

// ── set_ttl / get_ttl ────────────────────────────────────────────────────────

#[tauri::command]
pub fn set_ttl(peer_id: String, ttl_seconds: i64, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    if let Some(ref conn) = *db {
        db::set_ttl(conn, &peer_id, ttl_seconds)?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_ttl(peer_id: String, app: AppHandle) -> i64 {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    match db.as_ref() {
        Some(conn) => db::get_ttl(conn, &peer_id),
        None => 0,
    }
}

// ── group_has_sender_key ──────────────────────────────────────────────────────

/// Returns true if this client has distributed (and stored) a sender chain key for the group.
#[tauri::command]
pub fn group_has_sender_key(group_id: String, app: AppHandle) -> bool {
    let state = app.state::<AppState>();
    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        match guard.as_ref() {
            Some(id) => id.user_id.clone(),
            None => return false,
        }
    };
    let db = state.db.lock().unwrap();
    db.as_ref()
        .and_then(|c| crate::db::get_sender_chain(c, &group_id, &my_user_id))
        .map(|(_, _, distributed)| distributed)
        .unwrap_or(false)
}

// ── edit_message ──────────────────────────────────────────────────────────────

fn build_edit_payload(msg_ts: i64, msg_from: &str, new_text: &str) -> String {
    serde_json::json!({
        "v": 1,
        "edit": { "msg_ts": msg_ts, "msg_from": msg_from, "text": new_text }
    })
    .to_string()
}

#[tauri::command]
pub async fn edit_message(
    peer_id: String,
    msg_ts: i64,
    new_text: String,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;

    // Apply edit locally first.
    {
        let now = store::now_ms();
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            db::apply_edit(conn, &peer_id, msg_ts, &new_text, now)?;
        }
    }

    let payload = build_edit_payload(msg_ts, &my_user_id, &new_text);

    // Determine recipients: group fan-out or single DM.
    let recipients: Vec<String> = {
        let groups = state.groups.lock().unwrap();
        if let Some(group) = groups.get(&peer_id) {
            group.members.iter()
                .filter(|m| m.user_id != my_user_id)
                .map(|m| m.user_id.clone())
                .collect()
        } else {
            vec![peer_id.clone()]
        }
    };

    for recipient in &recipients {
        prepare_session(recipient.clone(), app.clone()).await.ok();

        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);

        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(recipient.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope { x3dh: x3dh_header, pq_ct, message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(recipient, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope { session_id: [0u8; 16], message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(recipient, &wf, msg_id)
                }
            };
            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };

        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }

        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    Ok(())
}

fn build_delete_payload(msg_ts: i64, msg_from: &str) -> String {
    serde_json::json!({ "v": 1, "del": { "msg_ts": msg_ts, "msg_from": msg_from } }).to_string()
}

#[tauri::command]
pub async fn delete_message(
    peer_id: String,
    msg_ts: i64,
    for_all: bool,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();

    // Always delete locally first.
    {
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            db::delete_message(conn, &peer_id, msg_ts)?;
        }
    }

    if !for_all {
        return Ok(());
    }

    let my_user_id = {
        let guard = state.identity.lock().unwrap();
        guard.as_ref().ok_or("not registered")?.user_id.clone()
    };
    let session_key = state.session_key.lock().unwrap().clone().ok_or("not unlocked")?;

    let payload = build_delete_payload(msg_ts, &my_user_id);

    let recipients: Vec<String> = {
        let groups = state.groups.lock().unwrap();
        if let Some(group) = groups.get(&peer_id) {
            group.members.iter()
                .filter(|m| m.user_id != my_user_id)
                .map(|m| m.user_id.clone())
                .collect()
        } else {
            vec![peer_id.clone()]
        }
    };

    for recipient in &recipients {
        prepare_session(recipient.clone(), app.clone()).await.ok();

        let msg_id = state.msg_counter.fetch_add(1, Ordering::Relaxed);

        let (routing_frame, encrypted_sessions) = {
            let mut sessions = state.sessions.lock().unwrap();
            let session = match sessions.remove(recipient.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let routing_frame = match session {
                PeerSession::AlicePending { x3dh_header, pq_ct, mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&InitEnvelope { x3dh: x3dh_header, pq_ct, message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(recipient, &wf, msg_id)
                }
                PeerSession::Established { mut ratchet } => {
                    let msg = ratchet.encrypt(payload.as_bytes(), AD);
                    let wf = wire::encode(&MessageEnvelope { session_id: [0u8; 16], message: msg, ad: AD.to_vec() })
                        .map_err(|e| e.to_string())?;
                    sessions.insert(recipient.clone(), PeerSession::Established { ratchet });
                    client::build_routing_frame(recipient, &wf, msg_id)
                }
            };
            let encrypted = store::extract_and_encrypt(&sessions, &session_key);
            (routing_frame, encrypted)
        };

        if let Some(tx) = state.ws_tx.lock().unwrap().as_ref() {
            let _ = tx.send(routing_frame);
        }

        if !encrypted_sessions.is_empty() {
            if let Ok(dir) = store::data_dir(&app) {
                tokio::spawn(store::save_sessions(dir, encrypted_sessions));
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub fn get_edit_history(msg_id: i64, app: AppHandle) -> Vec<db::EditHistoryEntry> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    match db.as_ref() {
        Some(conn) => db::get_edit_history(conn, msg_id),
        None => vec![],
    }
}

// ── export_chat ───────────────────────────────────────────────────────────────

fn ts_to_local_string(ts_ms: i64) -> String {
    let secs = ts_ms / 1000;
    let hours = (secs / 3600) % 24;
    let minutes = (secs / 60) % 60;
    let year = 1970 + secs / 31_536_000;
    format!("{year}-??-?? {hours:02}:{minutes:02}")
}

fn format_chat_json(
    messages: &[db::ExportMessage],
    reactions: &[db::ExportReaction],
    peer_names: &std::collections::HashMap<String, String>,
) -> String {
    let msgs_json: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let sender = m.sender_id.as_deref()
                .and_then(|sid| peer_names.get(sid))
                .cloned()
                .unwrap_or_else(|| m.direction.clone());
            let rxns: Vec<serde_json::Value> = reactions
                .iter()
                .filter(|r| r.msg_ts == m.ts)
                .map(|r| serde_json::json!({
                    "reactor": peer_names.get(&r.reactor_id).cloned().unwrap_or(r.reactor_id.clone()),
                    "emoji": r.emoji
                }))
                .collect();
            serde_json::json!({
                "ts": m.ts,
                "direction": m.direction,
                "sender": sender,
                "text": m.plain,
                "reactions": rxns,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "messages": msgs_json }))
        .unwrap_or_default()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}

fn format_chat_html(
    messages: &[db::ExportMessage],
    reactions: &[db::ExportReaction],
    peer_names: &std::collections::HashMap<String, String>,
) -> String {
    let mut html = String::from(
        "<!DOCTYPE html><html><head><meta charset='utf-8'>\
         <style>body{font-family:sans-serif;max-width:800px;margin:0 auto;padding:20px}\
         .msg{margin:10px 0;padding:10px;border-radius:8px}\
         .sent{background:#dcf8c6;margin-left:20%}\
         .received{background:#fff;margin-right:20%;border:1px solid #eee}\
         .rxn{font-size:12px;color:#666;margin-top:4px}</style></head><body>",
    );
    for m in messages {
        let sender = m.sender_id.as_deref()
            .and_then(|sid| peer_names.get(sid))
            .cloned()
            .unwrap_or_else(|| m.direction.clone());
        let cls = if m.direction == "sent" { "sent" } else { "received" };
        html.push_str(&format!(
            "<div class='msg {cls}'><strong>{}</strong><br>{}</div>",
            html_escape(&sender), html_escape(&m.plain)
        ));
        let rxns: Vec<_> = reactions.iter().filter(|r| r.msg_ts == m.ts).collect();
        if !rxns.is_empty() {
            html.push_str("<div class='rxn'>");
            for r in rxns {
                let name = peer_names.get(&r.reactor_id).cloned().unwrap_or(r.reactor_id.clone());
                html.push_str(&format!("{} {} &nbsp;", r.emoji, html_escape(&name)));
            }
            html.push_str("</div>");
        }
    }
    html.push_str("</body></html>");
    html
}

fn format_chat_markdown(
    messages: &[db::ExportMessage],
    reactions: &[db::ExportReaction],
    peer_names: &std::collections::HashMap<String, String>,
) -> String {
    let mut md = String::from("# Chat Export\n\n");
    for m in messages {
        let sender = m.sender_id.as_deref()
            .and_then(|sid| peer_names.get(sid))
            .cloned()
            .unwrap_or_else(|| m.direction.clone());
        md.push_str(&format!("**{}**\n> {}\n\n", sender, m.plain));
        let rxns: Vec<_> = reactions.iter().filter(|r| r.msg_ts == m.ts).collect();
        for r in rxns {
            let name = peer_names.get(&r.reactor_id).cloned().unwrap_or(r.reactor_id.clone());
            md.push_str(&format!("{} _{}_  \n", r.emoji, name));
        }
    }
    md
}

fn encrypt_export(content: &[u8], password: &str) -> Result<Vec<u8>, String> {
    use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce_bytes), content)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(4 + 16 + 12 + ciphertext.len());
    out.extend_from_slice(b"CEXP");
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

#[tauri::command]
pub fn export_chat(
    peer_id: String,
    format: String,
    encrypted: bool,
    password: Option<String>,
    app: AppHandle,
) -> Result<Vec<u8>, String> {
    let state = app.state::<AppState>();
    let (messages, reactions) = {
        let db = state.db.lock().unwrap();
        match db.as_ref() {
            Some(conn) => db::load_for_export(conn, &peer_id),
            None => return Err("DB not open".into()),
        }
    };
    let peer_names = state.peer_names.lock().unwrap().clone();

    let content = match format.as_str() {
        "json"     => format_chat_json(&messages, &reactions, &peer_names),
        "html"     => format_chat_html(&messages, &reactions, &peer_names),
        "markdown" => format_chat_markdown(&messages, &reactions, &peer_names),
        _          => return Err("invalid format".into()),
    };

    if encrypted {
        let pw = password.ok_or("password required for encrypted export")?;
        encrypt_export(content.as_bytes(), &pw)
    } else {
        Ok(content.into_bytes())
    }
}

// ── Server URL config ─────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_server_url(state: tauri::State<AppState>) -> String {
    state.server_url.clone()
}

/// Write a new server URL to the config file. Restart the app to apply.
#[tauri::command]
pub fn set_server_url(url: String) -> Result<(), String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if url.is_empty() {
        return Err("URL cannot be empty".into());
    }
    server_url_config_path()
        .map_err(|e| e.to_string())
        .and_then(|p| {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&p, &url).map_err(|e| e.to_string())
        })
}

/// Platform-specific path to server_url.txt config file.
pub fn server_url_config_path() -> std::io::Result<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        Ok(base.join("com.veto.app").join("server_url.txt"))
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("com.veto.app")
            .join("server_url.txt"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        Ok(home
            .join(".config")
            .join("com.veto.app")
            .join("server_url.txt"))
    }
}

// ── Pinned messages ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn pin_message(
    peer_id: String,
    msg_ts: i64,
    msg_from: String,
    msg_text: String,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    if let Some(conn) = db.as_ref() {
        db::pin_message(conn, &peer_id, msg_ts, &msg_from, &msg_text, store::now_ms());
    }
    Ok(())
}

#[tauri::command]
pub async fn unpin_message(
    peer_id: String,
    msg_ts: i64,
    msg_from: String,
    app: AppHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    if let Some(conn) = db.as_ref() {
        db::unpin_message(conn, &peer_id, msg_ts, &msg_from);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_pinned_messages(
    peer_id: String,
    app: AppHandle,
) -> Result<Vec<db::PinnedMsg>, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    Ok(db.as_ref().map(|c| db::get_pinned_messages(c, &peer_id)).unwrap_or_default())
}

// ── Saved messages ────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct SavedNote {
    pub ts:   i64,
    pub text: String,
}

#[tauri::command]
pub async fn save_note(text: String, app: AppHandle) -> Result<i64, String> {
    let state = app.state::<AppState>();
    let ts = store::now_ms();
    let key = {
        let sk = state.session_key.lock().unwrap();
        sk.clone().ok_or("not unlocked")?
    };
    let (nonce, ct) = store::encrypt_content(&text, &key);
    let db = state.db.lock().unwrap();
    if let Some(conn) = db.as_ref() {
        db::save_note(conn, &nonce, &ct, &text, ts);
    }
    Ok(ts)
}

// ── update_profile ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn update_profile(display_name: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (server_url, token, user_id) = {
        let guard = state.identity.lock().unwrap();
        let id = guard.as_ref().ok_or("not registered")?;
        (state.server_url.clone(), id.token.clone(), id.user_id.clone())
    };

    state
        .http
        .patch(format!("{}/api/v1/users/me", server_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&serde_json::json!({ "display_name": display_name }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    // Update local display name in peer names cache via event
    app.emit("profile_updated", serde_json::json!({ "user_id": user_id, "display_name": display_name }))
        .ok();

    Ok(())
}

// ── get_chat_stats ────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct PeerStats {
    pub peer_id: String,
    pub msg_count: i64,
    pub sent_count: i64,
    pub recv_count: i64,
    pub file_count: i64,
    pub file_bytes: i64,
    pub oldest_ts: Option<i64>,
    pub newest_ts: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct ChatStats {
    pub peers: Vec<PeerStats>,
    pub total_msgs: i64,
    pub total_file_bytes: i64,
    pub db_size_bytes: i64,
}

#[tauri::command]
pub fn get_chat_stats(app: AppHandle) -> Result<ChatStats, String> {
    let state = app.state::<AppState>();
    let db = state.db.lock().unwrap();
    let conn = db.as_ref().ok_or("db not open")?;

    // Per-peer stats
    let mut stmt = conn.prepare(
        "SELECT peer_id, COUNT(*) as total,
                SUM(CASE WHEN direction='sent' THEN 1 ELSE 0 END) as sent,
                SUM(CASE WHEN direction<>'sent' THEN 1 ELSE 0 END) as recv,
                SUM(CASE WHEN file_id IS NOT NULL THEN 1 ELSE 0 END) as files,
                COALESCE(SUM(CASE WHEN file_size IS NOT NULL THEN file_size ELSE 0 END), 0) as fbytes,
                MIN(ts) as oldest, MAX(ts) as newest
         FROM messages GROUP BY peer_id ORDER BY total DESC",
    ).map_err(|e| e.to_string())?;

    let peers: Vec<PeerStats> = stmt.query_map([], |row| {
        Ok(PeerStats {
            peer_id:    row.get(0)?,
            msg_count:  row.get(1)?,
            sent_count: row.get(2)?,
            recv_count: row.get(3)?,
            file_count: row.get(4)?,
            file_bytes: row.get(5)?,
            oldest_ts:  row.get(6)?,
            newest_ts:  row.get(7)?,
        })
    }).map_err(|e| e.to_string())?
      .filter_map(|r| r.ok())
      .collect();

    let total_msgs: i64 = peers.iter().map(|p| p.msg_count).sum();
    let total_file_bytes: i64 = peers.iter().map(|p| p.file_bytes).sum();

    // SQLite DB file size
    let db_size_bytes: i64 = conn.query_row(
        "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        [], |row| row.get(0)
    ).unwrap_or(0);

    Ok(ChatStats { peers, total_msgs, total_file_bytes, db_size_bytes })
}

// ── set_screen_capture_protection ────────────────────────────────────────────

#[tauri::command]
pub async fn set_screen_capture_protection(
    enabled: bool,
    app: AppHandle,
) -> Result<(), String> {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("main") {
        window.set_content_protected(enabled).map_err(|e| e.to_string())?;
    }
    Ok(())
}
