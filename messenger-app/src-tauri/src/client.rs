use std::collections::HashMap;

use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, Key, KeyInit, Nonce};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream};

use messenger_crypto::{
    error::CryptoError,
    keys::{IdentityKeyPair, OneTimePreKey, SignedPreKey},
    ratchet::RatchetState,
    wire::{self, InitEnvelope, MessageEnvelope},
    x3dh::x3dh_receive,
};

use tauri_plugin_notification::NotificationExt;

use crate::{store, AppState, PeerSession};

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("wire decode error")]
    Wire,
    #[error("crypto error: {0}")]
    Crypto(#[from] messenger_crypto::error::CryptoError),
    #[error("invalid UTF-8 in plaintext")]
    Utf8(#[from] std::string::FromUtf8Error),
}

// ── Typing control frame ──────────────────────────────────────────────────────

/// Magic bytes that mark a typing indicator frame.  Not a valid postcard frame
/// (length field 0xCAFE0100 = ~3.4 GB, rejected by the parser), so safe to use
/// as an out-of-band signal without any server changes.
pub const TYPING_MAGIC: &[u8] = &[0xCA, 0xFE, 0x01, 0x00];

/// Magic prefix for delivery ACK frames sent by the server back to the sender.
/// Format: [ACK_MAGIC: 4B][msg_id: 4B BE u32] = 8 bytes total, no sender prefix.
pub const ACK_MAGIC: &[u8] = &[0xCA, 0xFE, 0x02, 0x00];

/// Magic bytes for a read receipt (sender → peer). Triggers blue ✓✓ on the sender's UI.
pub const READ_MAGIC: &[u8] = &[0xCA, 0xFE, 0x03, 0x00];

/// Magic bytes for a Sender Key group broadcast frame.
/// Sent client → server; server prepends verified sender_id and forwards to all group members.
/// Received as: [4B sid_len][sid][GROUP_MAGIC][4B gid_len][gid][4B counter][ciphertext]
pub const GROUP_MAGIC: &[u8] = &[0xCA, 0xFE, 0x04, 0x00];

/// Magic bytes for a group read receipt frame.
/// Format: [GROUP_READ_MAGIC][4B gid_len][gid UTF-8][8B ts_ms BE i64]
/// Server routes the same way as GROUP_MAGIC (fan-out to all group members).
pub const GROUP_READ_MAGIC: &[u8] = &[0xCA, 0xFE, 0x05, 0x00];

/// Server → client: a member has left a group.
/// Format: [GROUP_MEMBER_LEFT_MAGIC: 4B][u32_BE gid_len][gid UTF-8][u32_BE uid_len][leaver_uid UTF-8]
/// No sender_id prefix — authenticated server-sent notification.
pub const GROUP_MEMBER_LEFT_MAGIC: &[u8] = &[0xCA, 0xFE, 0x06, 0x00];

// ── Frame helpers ─────────────────────────────────────────────────────────────

/// Parse `[4B BE: len][str_bytes][rest]` → `(str, rest)`.
pub fn parse_u32_len_prefix(data: &[u8]) -> Option<(&str, &[u8])> {
    if data.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
    if data.len() < 4 + len {
        return None;
    }
    let s = std::str::from_utf8(&data[4..4 + len]).ok()?;
    Some((s, &data[4 + len..]))
}

/// Build outgoing routing frame:
/// `[4B BE: rid_len][rid][4B BE: msg_id][wire_frame]`
///
/// sender_id is NOT included — the server authenticates the sender via the WS
/// auth token and prepends the verified sender_id before forwarding to the recipient.
///
/// `msg_id` = 0 for fire-and-forget frames (typing/read receipts); non-zero for
/// messages that expect a delivery ACK from the server.
pub fn build_routing_frame(recipient_id: &str, wire_frame: &[u8], msg_id: u32) -> Vec<u8> {
    let rid = recipient_id.as_bytes();
    let mut frame = Vec::with_capacity(4 + rid.len() + 4 + wire_frame.len());
    frame.extend_from_slice(&(rid.len() as u32).to_be_bytes());
    frame.extend_from_slice(rid);
    frame.extend_from_slice(&msg_id.to_be_bytes());
    frame.extend_from_slice(wire_frame);
    frame
}

// ── Message payload parsing ───────────────────────────────────────────────────

struct ParsedMessage {
    text: String,
    gid: Option<String>,
    r_ts: Option<i64>,
    r_from: Option<String>,
    r_text: Option<String>,
    file_id: Option<String>,
    file_key: Option<Vec<u8>>,
    file_name: Option<String>,
    file_mime: Option<String>,
    file_size: Option<i64>,
    /// Decrypted thumbnail JPEG bytes (200×200), None for non-image or old messages.
    thumb_data: Option<Vec<u8>>,
    /// Sender's canonical timestamp from payload ts field (only present in v1 messages).
    sender_ts: Option<i64>,
    /// Set when the message is a reaction (not a regular message).
    reaction: Option<ParsedReaction>,
    /// Set when the message is an edit.
    edit: Option<ParsedEdit>,
    /// Set when the message is a delete request.
    del: Option<ParsedDelete>,
    /// Set when the message distributes a sender chain key.
    skd: Option<ParsedSKD>,
}

struct ParsedEdit {
    msg_ts:   i64,
    msg_from: String,
    new_text: String,
    /// peer_id context (gid for groups, else sender_id).
    peer_id:  String,
}

struct ParsedDelete {
    msg_ts:   i64,
    msg_from: String,
    peer_id:  String,
}

/// Sender Key Distribution message: distributes the sender's chain key for a group.
struct ParsedSKD {
    gid:       String,
    chain_key: [u8; 32],
}

struct ParsedReaction {
    msg_ts:   i64,
    msg_from: String,
    emoji:    String,
    add:      bool,
    /// peer_id context (gid for groups, else sender_id — filled in by ws_loop caller).
    peer_id:  String,
}

/// Decode the decrypted plaintext. Supports:
/// - Plain string (legacy): returned as-is.
/// - JSON v1 text message: `{"v":1,"text":"...","r":{...}}`
/// - JSON v1 file message: `{"v":1,"file_id":"...","key":[...],"name":"...","mime":"...","size":N}`
/// - JSON v1 reaction:     `{"v":1,"react":{"msg_ts":N,"msg_from":"...","emoji":"...","add":bool}}`
fn parse_message_payload(s: &str, fallback_peer_id: &str) -> ParsedMessage {
    let empty_reaction = ParsedMessage {
        text: String::new(), gid: None,
        r_ts: None, r_from: None, r_text: None,
        file_id: None, file_key: None, file_name: None, file_mime: None, file_size: None,
        thumb_data: None,
        sender_ts: None,
        reaction: None,
        edit: None,
        del: None,
        skd: None,
    };
    let plain = |text: String| ParsedMessage { text, ..empty_reaction.clone() };
    if !s.starts_with('{') {
        return plain(s.to_string());
    }
    #[derive(serde::Deserialize)]
    struct V1Reply { ts: i64, f: String, t: String }
    #[derive(serde::Deserialize)]
    struct V1React { msg_ts: i64, msg_from: String, emoji: String, add: bool }
    #[derive(serde::Deserialize)]
    struct V1Edit { msg_ts: i64, msg_from: String, text: String }
    #[derive(serde::Deserialize)]
    struct V1Del  { msg_ts: i64, msg_from: String }
    #[derive(serde::Deserialize)]
    struct V1Skd { gid: String, ck: Vec<u8> }   // ck = 32-byte chain key
    #[derive(serde::Deserialize)]
    struct V1Msg {
        v: Option<u8>,
        text: Option<String>,
        ts: Option<i64>,
        gid: Option<String>,
        r: Option<V1Reply>,
        file_id: Option<String>,
        key: Option<Vec<u8>>,
        name: Option<String>,
        mime: Option<String>,
        size: Option<i64>,
        thn: Option<Vec<u8>>,
        thc: Option<Vec<u8>>,
        react: Option<V1React>,
        edit: Option<V1Edit>,
        del: Option<V1Del>,
        skd: Option<V1Skd>,
    }
    let Ok(msg) = serde_json::from_str::<V1Msg>(s) else {
        return plain(s.to_string());
    };
    if msg.v != Some(1) {
        return plain(s.to_string());
    }
    let gid = msg.gid;
    // Reaction message.
    if let Some(react) = msg.react {
        let peer_id = gid.clone().unwrap_or_else(|| fallback_peer_id.to_string());
        return ParsedMessage {
            reaction: Some(ParsedReaction {
                msg_ts: react.msg_ts,
                msg_from: react.msg_from,
                emoji: react.emoji,
                add: react.add,
                peer_id,
            }),
            gid,
            ..empty_reaction
        };
    }
    // Edit message.
    if let Some(e) = msg.edit {
        let peer_id = gid.clone().unwrap_or_else(|| fallback_peer_id.to_string());
        return ParsedMessage {
            edit: Some(ParsedEdit {
                msg_ts:   e.msg_ts,
                msg_from: e.msg_from,
                new_text: e.text,
                peer_id,
            }),
            gid,
            ..empty_reaction
        };
    }
    // Delete message.
    if let Some(d) = msg.del {
        let peer_id = gid.clone().unwrap_or_else(|| fallback_peer_id.to_string());
        return ParsedMessage {
            del: Some(ParsedDelete {
                msg_ts:   d.msg_ts,
                msg_from: d.msg_from,
                peer_id,
            }),
            gid,
            ..empty_reaction
        };
    }
    // Sender Key Distribution: store chain key for group, don't surface as UI message.
    if let Some(s) = msg.skd {
        if let Ok(arr) = <[u8; 32]>::try_from(s.ck.as_slice()) {
            return ParsedMessage {
                skd: Some(ParsedSKD { gid: s.gid, chain_key: arr }),
                ..empty_reaction
            };
        }
    }
    // File message — all fields must be present.
    if let (Some(fid), Some(key), Some(name), Some(mime), Some(size)) =
        (msg.file_id, msg.key, msg.name, msg.mime, msg.size)
    {
        // Decrypt thumbnail if sender included it (separate nonce from file nonce).
        let thumb_data = if let (Some(tn), Some(tc)) = (msg.thn, msg.thc) {
            if tn.len() == 12 && key.len() == 32 {
                ChaCha20Poly1305::new(Key::from_slice(&key))
                    .decrypt(Nonce::from_slice(&tn), tc.as_slice())
                    .ok()
            } else {
                None
            }
        } else {
            None
        };
        return ParsedMessage {
            text: String::new(),
            gid,
            r_ts: None, r_from: None, r_text: None,
            file_id: Some(fid),
            file_key: Some(key),
            file_name: Some(name),
            file_mime: Some(mime),
            file_size: Some(size),
            thumb_data,
            sender_ts: msg.ts,
            reaction: None,
            edit: None,
            del: None,
            skd: None,
        };
    }
    // Text message.
    if let Some(text) = msg.text {
        let sender_ts = msg.ts;
        let (r_ts, r_from, r_text) = match msg.r {
            Some(r) => (Some(r.ts), Some(r.f), Some(r.t)),
            None    => (None, None, None),
        };
        return ParsedMessage {
            text, gid,
            r_ts, r_from, r_text,
            file_id: None, file_key: None, file_name: None, file_mime: None, file_size: None,
            thumb_data: None,
            sender_ts,
            reaction: None,
            edit: None,
            del: None,
            skd: None,
        };
    }
    plain(s.to_string())
}

impl Clone for ParsedMessage {
    fn clone(&self) -> Self {
        ParsedMessage {
            text: self.text.clone(),
            gid: self.gid.clone(),
            r_ts: self.r_ts,
            r_from: self.r_from.clone(),
            r_text: self.r_text.clone(),
            file_id: self.file_id.clone(),
            file_key: self.file_key.clone(),
            file_name: self.file_name.clone(),
            file_mime: self.file_mime.clone(),
            file_size: self.file_size,
            thumb_data: self.thumb_data.clone(),
            sender_ts: self.sender_ts,
            reaction: None,
            edit: None,
            del: None,
            skd: None,
        }
    }
}

// ── Crypto dispatch ───────────────────────────────────────────────────────────

/// Decrypt one incoming wire frame. Pure sync — no `.await` inside.
/// Must be called with the sessions lock already held (lock passed in).
///
/// Returns `(plaintext, peer_ik, consumed_opk_id)`:
/// - `peer_ik` — `Some(x25519_key_bytes)` when processing an InitEnvelope (Bob side).
/// - `consumed_opk_id` — `Some(id)` when the InitEnvelope used a one-time prekey;
///   caller must remove that OPK from local storage so it is never reused.
pub fn handle_incoming_frame(
    sender_id: &str,
    wire_frame: &[u8],
    sessions: &mut HashMap<String, PeerSession>,
    signing_key_bytes: [u8; 32],
    spk_secret_bytes: [u8; 32],
    spk_id: u32,
    opk_secrets: &[(u32, [u8; 32])],
    pq_spk_secret_bytes: Vec<u8>,
) -> Result<(String, Option<[u8; 32]>, Option<u32>), ClientError> {
    use ed25519_dalek::SigningKey;
    use x25519_dalek::StaticSecret;

    match sessions.remove(sender_id) {
        // Bob receiving Alice's first message, or both sides initiated simultaneously.
        None | Some(PeerSession::AlicePending { .. }) => {
            let (envelope, _): (InitEnvelope, _) =
                wire::decode(wire_frame).map_err(|_| ClientError::Wire)?;

            let peer_ik = envelope.x3dh.ik_a.to_bytes();
            let identity = IdentityKeyPair { signing: SigningKey::from_bytes(&signing_key_bytes) };
            let spk = SignedPreKey { secret: StaticSecret::from(spk_secret_bytes), id: spk_id };

            // Look up OPK private key if Alice claims she used one.
            let (opk_holder, used_opk_id) = match envelope.x3dh.opk_id {
                Some(opk_id) => {
                    match opk_secrets.iter().find(|(id, _)| *id == opk_id).map(|(_, b)| *b) {
                        Some(bytes) => {
                            let opk = OneTimePreKey { secret: StaticSecret::from(bytes), id: opk_id };
                            (Some(opk), Some(opk_id))
                        }
                        None => return Err(ClientError::Crypto(CryptoError::NoOneTimePrekey)),
                    }
                }
                None => (None, None),
            };

            let pq_dk = if pq_spk_secret_bytes.is_empty() { None } else { Some(pq_spk_secret_bytes.as_slice()) };
            let secret = x3dh_receive(
                &identity, &spk, opk_holder.as_ref(), &envelope.x3dh,
                envelope.pq_ct.as_deref(),
                pq_dk,
            )?;

            let mut ratchet = RatchetState::init_bob(&secret, StaticSecret::from(spk_secret_bytes));
            let plaintext = ratchet.decrypt(&envelope.message, &envelope.ad)?;
            sessions.insert(sender_id.to_string(), PeerSession::Established { ratchet });
            Ok((String::from_utf8(plaintext)?, Some(peer_ik), used_opk_id))
        }

        Some(PeerSession::Established { mut ratchet }) => {
            let (envelope, _): (MessageEnvelope, _) =
                wire::decode(wire_frame).map_err(|_| ClientError::Wire)?;
            let plaintext = ratchet.decrypt(&envelope.message, &envelope.ad)?;
            sessions.insert(sender_id.to_string(), PeerSession::Established { ratchet });
            Ok((String::from_utf8(plaintext)?, None, None))
        }
    }
}

// ── Sender Key helpers ────────────────────────────────────────────────────────

/// Build a GROUP_BROADCAST frame to send to the server:
/// `[GROUP_MAGIC][4B gid_len][gid UTF-8][4B counter][ciphertext]`
pub fn build_group_broadcast_frame(gid: &str, payload: &[u8]) -> Vec<u8> {
    let gid_bytes = gid.as_bytes();
    let mut frame = Vec::with_capacity(4 + 4 + gid_bytes.len() + payload.len());
    frame.extend_from_slice(GROUP_MAGIC);
    frame.extend_from_slice(&(gid_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(gid_bytes);
    frame.extend_from_slice(payload);
    frame
}

/// Build a group read receipt frame: `[GROUP_READ_MAGIC][4B gid_len][gid][8B ts_ms BE]`
pub fn build_group_read_frame(gid: &str, ts_ms: i64) -> Vec<u8> {
    let gid_bytes = gid.as_bytes();
    let mut frame = Vec::with_capacity(4 + 4 + gid_bytes.len() + 8);
    frame.extend_from_slice(GROUP_READ_MAGIC);
    frame.extend_from_slice(&(gid_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(gid_bytes);
    frame.extend_from_slice(&ts_ms.to_be_bytes());
    frame
}

/// Handle an incoming group read receipt frame in ws_loop.
fn handle_group_read(app: &AppHandle, sender_id: &str, wire_frame: &[u8]) {
    // wire_frame = [GROUP_READ_MAGIC:4][4B gid_len][gid][8B ts_ms]
    let after_magic = &wire_frame[4..];
    if after_magic.len() < 4 {
        return;
    }
    let gid_len = u32::from_be_bytes(after_magic[..4].try_into().unwrap()) as usize;
    if after_magic.len() < 4 + gid_len + 8 {
        return;
    }
    let gid = match std::str::from_utf8(&after_magic[4..4 + gid_len]) {
        Ok(s) => s,
        Err(_) => return,
    };
    let ts_bytes: [u8; 8] = after_magic[4 + gid_len..4 + gid_len + 8].try_into().unwrap();
    let ts = i64::from_be_bytes(ts_bytes);

    {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        if let Some(ref conn) = *db {
            let _ = crate::db::set_group_read_mark(conn, gid, sender_id, ts);
        }
    }
    use tauri::Emitter;
    app.emit("group_read", json!({ "gid": gid, "from": sender_id, "ts": ts })).ok();
}

/// Handle an incoming Sender Key group broadcast frame in ws_loop.
fn handle_group_broadcast(app: &AppHandle, sender_id: &str, wire_frame: &[u8]) {
    // wire_frame = [GROUP_MAGIC:4][4B gid_len][gid][sk_payload]
    let after_magic = &wire_frame[4..];
    if after_magic.len() < 4 {
        return;
    }
    let gid_len = u32::from_be_bytes(after_magic[..4].try_into().unwrap()) as usize;
    if after_magic.len() < 4 + gid_len {
        return;
    }
    let gid = match std::str::from_utf8(&after_magic[4..4 + gid_len]) {
        Ok(s) => s,
        Err(_) => return,
    };
    let sk_payload = &after_magic[4 + gid_len..];

    // Look up the sender's chain key for this group.
    let chain_key_opt = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.as_ref().and_then(|c| crate::db::get_sender_chain(c, gid, sender_id))
    };
    let Some((chain_key, _, _)) = chain_key_opt else {
        tracing::warn!(gid, sender_id, "no sender chain key for group broadcast — dropping");
        return;
    };

    let Ok((plaintext_bytes, _counter)) = messenger_crypto::sender_keys::decrypt(&chain_key, sk_payload) else {
        tracing::warn!(gid, sender_id, "sender key decrypt failed");
        return;
    };
    let Ok(raw_text) = String::from_utf8(plaintext_bytes) else {
        return;
    };

    let parsed = parse_message_payload(&raw_text, sender_id);
    let ts = store::now_ms();

    // Persist to local DB.
    {
        let state = app.state::<AppState>();
        let session_key = state.session_key.lock().unwrap().clone();
        if let Some(ref key) = session_key {
            let db = state.db.lock().unwrap();
            if let Some(ref conn) = *db {
                let (nonce, ct) = store::encrypt_content(&parsed.text, key);
                let _ = crate::db::insert_group_received(
                    conn,
                    gid,
                    sender_id,
                    ts,
                    &nonce,
                    &ct,
                    None,  // no wire_hash (SK messages aren't DR-wrapped)
                    parsed.r_ts,
                    parsed.r_from.as_deref(),
                    parsed.r_text.as_deref(),
                    None,
                    None,
                    None,
                    None,
                    Some(parsed.text.as_str()), None,
                );
            }
        }
    }

    app.emit("group_message", json!({
        "gid":          gid,
        "from":         sender_id,
        "text":         parsed.text,
        "ts":           ts,
        "reply_to_ts":  parsed.r_ts,
        "reply_to_from": parsed.r_from,
        "reply_to_text": parsed.r_text,
        "file_id":      parsed.file_id,
        "file_key":     parsed.file_key,
        "file_name":    parsed.file_name,
        "file_mime":    parsed.file_mime,
        "file_size":    parsed.file_size,
        "sk":           true,
    })).ok();

    // System notification.
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    let muted = {
        let state = app.state::<AppState>();
        let db = state.db.lock().unwrap();
        db.as_ref()
            .map(|c| crate::db::get_mute(c, gid, store::now_ms()).is_muted)
            .unwrap_or(false)
    };
    if !focused && !muted {
        let sender_label = {
            let state = app.state::<AppState>();
            let names = state.peer_names.lock().unwrap();
            names.get(sender_id).cloned().unwrap_or_else(|| sender_id[..8.min(sender_id.len())].to_string())
        };
        let _ = app.notification().builder()
            .title(format!("Group message from {sender_label}"))
            .body(if parsed.text.is_empty() { "📎 File" } else { parsed.text.as_str() })
            .show();
    }
}

/// Handle a server-sent GROUP_MEMBER_LEFT_MAGIC frame (no sender_id prefix).
/// Deletes the leaver's sender chain, removes them from the in-memory group cache,
/// then rotates our own sender key so the leaver can no longer decrypt future messages.
fn handle_group_member_left(app: &AppHandle, data: &[u8]) {
    let result = (|| -> Option<()> {
        let after_magic = data.get(4..)?;
        let (gid, rest) = parse_u32_len_prefix(after_magic)?;
        let (leaver_uid, _) = parse_u32_len_prefix(rest)?;

        let state = app.state::<AppState>();

        // Delete the leaver's cached sender chain so we stop accepting their messages.
        {
            let db = state.db.lock().unwrap();
            if let Some(conn) = db.as_ref() {
                crate::db::delete_sender_chain(conn, gid, leaver_uid);
            }
        }

        // Remove leaver from in-memory group cache BEFORE distribute_sender_key
        // so they do not receive the new key.
        {
            let mut groups = state.groups.lock().unwrap();
            if let Some(group) = groups.get_mut(gid) {
                group.members.retain(|m| m.user_id != leaver_uid);
            }
        }

        let gid_str = gid.to_string();
        let leaver_str = leaver_uid.to_string();

        app.emit("group_member_left", json!({ "groupId": &gid_str, "userId": &leaver_str })).ok();

        // Rotate our own sender key on a background task.
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::distribute_sender_key(gid_str, app2).await {
                tracing::warn!("sender key rotation after member-left failed: {e}");
            }
        });

        Some(())
    })();

    if result.is_none() {
        tracing::warn!("Failed to parse GROUP_MEMBER_LEFT frame");
    }
}

// ── WebSocket background task ─────────────────────────────────────────────────

/// Long-lived background task that owns an already-connected WebSocket stream.
/// Sends outgoing frames from `out_rx`, emits Tauri events for incoming frames.
pub async fn ws_loop(
    ws_stream: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    app_handle: AppHandle,
    out_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let (mut sink, mut stream) = ws_stream.split();
    let mut out_rx = out_rx;
    let data_dir = store::data_dir(&app_handle).ok();

    let send_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(WsMessage::Binary(frame.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(item) = stream.next().await {
        match item {
            Ok(WsMessage::Binary(data)) => {
                // Delivery ACK from server: [ACK_MAGIC: 4B][msg_id: 4B] = 8 bytes, no sender prefix.
                if data.len() == 8 && data[..4] == *ACK_MAGIC {
                    let msg_id = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    let state = app_handle.state::<AppState>();
                    // ACKed — no need to retry on reconnect.
                    state.outgoing_queue.lock().unwrap().retain(|(id, _)| *id != msg_id);
                    let peer_id_opt = {
                        let db = state.db.lock().unwrap();
                        db.as_ref().and_then(|conn| crate::db::set_delivered(conn, msg_id))
                    };
                    if let Some(peer_id) = peer_id_opt {
                        app_handle.emit("delivered", json!({ "id": msg_id, "peerId": peer_id })).ok();
                    }
                    continue;
                }

                // Member-left notification: server-sent, no sender_id prefix.
                if data.len() >= 4 && &data[..4] == GROUP_MEMBER_LEFT_MAGIC {
                    handle_group_member_left(&app_handle, &data);
                    continue;
                }

                let Some((sender_id, wire_frame)) = parse_u32_len_prefix(&data) else {
                    tracing::warn!("received frame with missing sender prefix, dropping");
                    continue;
                };
                let sender_id = sender_id.to_string();

                // Typing indicator: short magic frame, not a crypto message.
                if wire_frame == TYPING_MAGIC {
                    app_handle.emit("typing", json!({ "from": sender_id })).ok();
                    continue;
                }

                // Read receipt: peer has read all our messages.
                if wire_frame == READ_MAGIC {
                    {
                        let state = app_handle.state::<AppState>();
                        let db = state.db.lock().unwrap();
                        if let Some(ref conn) = *db {
                            let _ = crate::db::set_all_read(conn, &sender_id);
                        }
                    }
                    app_handle.emit("read", json!({ "from": sender_id })).ok();
                    continue;
                }

                // Group read receipt: [GROUP_READ_MAGIC][4B gid_len][gid][8B ts]
                if wire_frame.len() >= 4 && &wire_frame[..4] == GROUP_READ_MAGIC {
                    handle_group_read(&app_handle, &sender_id, wire_frame);
                    continue;
                }

                // Sender Key group broadcast: [GROUP_MAGIC][4B gid_len][gid][4B counter][ciphertext]
                if wire_frame.len() >= 4 && &wire_frame[..4] == GROUP_MAGIC {
                    handle_group_broadcast(&app_handle, &sender_id, wire_frame);
                    continue;
                }

                // Duplicate detection: hash the wire frame before attempting crypto.
                // If the same encrypted frame arrives twice (e.g., NATS + queue replay),
                // the second decrypt would fail (key consumed) and log a spurious error.
                // Checking the hash first lets us skip it cleanly.
                let wire_hash: [u8; 32] =
                    Sha256::new().chain_update(wire_frame).finalize().into();
                {
                    let state = app_handle.state::<AppState>();
                    let db = state.db.lock().unwrap();
                    if db
                        .as_ref()
                        .map(|c| crate::db::is_wire_frame_seen(c, &wire_hash))
                        .unwrap_or(false)
                    {
                        continue;
                    }
                }

                let (crypto_result, encrypted_sessions) = {
                    let state = app_handle.state::<AppState>();
                    let (signing_key_bytes, spk_secret_bytes, spk_id, opk_secrets, pq_spk_secret_bytes) = {
                        let guard = state.identity.lock().unwrap();
                        let id = guard.as_ref().expect("identity must exist while WS is active");
                        (
                            id.signing_key_bytes,
                            id.spk_secret_bytes,
                            id.spk_id,
                            id.opk_secret_bytes.clone(),
                            id.pq_spk_secret_bytes.clone(),
                        )
                    };
                    let session_key = state.session_key.lock().unwrap().clone();
                    let mut sessions = state.sessions.lock().unwrap();
                    let res = handle_incoming_frame(
                        &sender_id,
                        wire_frame,
                        &mut sessions,
                        signing_key_bytes,
                        spk_secret_bytes,
                        spk_id,
                        &opk_secrets,
                        pq_spk_secret_bytes,
                    );
                    let encrypted = match (res.is_ok(), session_key) {
                        (true, Some(ref key)) => store::extract_and_encrypt(&sessions, key),
                        _ => Vec::new(),
                    };
                    (res, encrypted)
                };

                match crypto_result {
                    Ok((raw_text, peer_ik, used_opk_id)) => {
                        // Store peer identity key for safety number verification (Bob side).
                        if let Some(ik) = peer_ik {
                            app_handle
                                .state::<AppState>()
                                .peer_identity_keys
                                .lock()
                                .unwrap()
                                .insert(sender_id.clone(), ik);
                        }

                        // Consumed OPK must be removed from local storage — it can never be
                        // reused (one-time). Remove from in-memory state and persist to disk.
                        if let Some(consumed_id) = used_opk_id {
                            {
                                let state = app_handle.state::<AppState>();
                                let mut guard = state.identity.lock().unwrap();
                                if let Some(ref mut id) = *guard {
                                    id.opk_secret_bytes.retain(|(opk_id, _)| *opk_id != consumed_id);
                                }
                            }
                            if let Some(ref dir) = data_dir {
                                let dir_clone = dir.clone();
                                tokio::spawn(async move {
                                    let path = dir_clone.join("identity.json");
                                    let Ok(bytes) = tokio::fs::read(&path).await else { return };
                                    let Ok(mut stored) =
                                        serde_json::from_slice::<store::StoredIdentity>(&bytes)
                                    else {
                                        return;
                                    };
                                    stored.opk_secrets.retain(|(id, _)| *id != consumed_id);
                                    let _ = store::save(&path, &stored).await;
                                });
                            }
                        }

                        let parsed = parse_message_payload(&raw_text, &sender_id);
                        let ts = parsed.sender_ts.unwrap_or_else(store::now_ms);

                        // Reaction message — store + emit then skip normal message handling.
                        if let Some(ref react) = parsed.reaction {
                            {
                                let state = app_handle.state::<AppState>();
                                let db = state.db.lock().unwrap();
                                if let Some(ref conn) = *db {
                                    if react.add {
                                        let _ = crate::db::add_reaction(
                                            conn,
                                            &react.peer_id,
                                            react.msg_ts,
                                            &react.msg_from,
                                            &sender_id,
                                            &react.emoji,
                                        );
                                    } else {
                                        let _ = crate::db::remove_reaction(
                                            conn,
                                            &react.peer_id,
                                            react.msg_ts,
                                            &react.msg_from,
                                            &sender_id,
                                        );
                                    }
                                }
                            }
                            app_handle.emit("reaction", json!({
                                "peer_id":   react.peer_id,
                                "msg_ts":    react.msg_ts,
                                "msg_from":  react.msg_from,
                                "reactor_id": sender_id,
                                "emoji":     react.emoji,
                                "add":       react.add,
                            })).ok();
                            if !encrypted_sessions.is_empty() {
                                if let Some(ref dir) = data_dir {
                                    tokio::spawn(store::save_sessions(dir.clone(), encrypted_sessions));
                                }
                            }
                            continue;
                        }

                        // SKD: store chain key, do not surface to UI.
                        if let Some(ref s) = parsed.skd {
                            {
                                let state = app_handle.state::<AppState>();
                                let db = state.db.lock().unwrap();
                                if let Some(ref conn) = *db {
                                    let _ = crate::db::set_sender_chain(
                                        conn, &s.gid, &sender_id, &s.chain_key, 0,
                                    );
                                }
                            }
                            if !encrypted_sessions.is_empty() {
                                if let Some(ref dir) = data_dir {
                                    tokio::spawn(store::save_sessions(dir.clone(), encrypted_sessions));
                                }
                            }
                            continue;
                        }

                        // Edit message — update DB + emit then skip normal handling.
                        if let Some(ref ed) = parsed.edit {
                            {
                                let state = app_handle.state::<AppState>();
                                let db = state.db.lock().unwrap();
                                if let Some(ref conn) = *db {
                                    let now = store::now_ms();
                                    if let Ok(Some(msg_id)) = crate::db::apply_edit(
                                        conn, &ed.peer_id, ed.msg_ts, &ed.new_text, now,
                                    ) {
                                        app_handle.emit("message_edited", json!({
                                            "peer_id":  ed.peer_id,
                                            "msg_id":   msg_id,
                                            "msg_ts":   ed.msg_ts,
                                            "new_text": ed.new_text,
                                            "edited_at": now,
                                        })).ok();
                                    }
                                }
                            }
                            if !encrypted_sessions.is_empty() {
                                if let Some(ref dir) = data_dir {
                                    tokio::spawn(store::save_sessions(dir.clone(), encrypted_sessions));
                                }
                            }
                            continue;
                        }

                        // Delete message — remove from DB + emit then skip normal handling.
                        if let Some(ref d) = parsed.del {
                            {
                                let state = app_handle.state::<AppState>();
                                let db = state.db.lock().unwrap();
                                if let Some(ref conn) = *db {
                                    if let Ok(Some(msg_id)) = crate::db::delete_message(
                                        conn, &d.peer_id, d.msg_ts,
                                    ) {
                                        app_handle.emit("message_deleted", json!({
                                            "peer_id":  d.peer_id,
                                            "msg_id":   msg_id,
                                            "msg_ts":   d.msg_ts,
                                            "msg_from": d.msg_from,
                                        })).ok();
                                    }
                                }
                            }
                            if !encrypted_sessions.is_empty() {
                                if let Some(ref dir) = data_dir {
                                    tokio::spawn(store::save_sessions(dir.clone(), encrypted_sessions));
                                }
                            }
                            continue;
                        }

                        // Persist to local DB before emitting; INSERT OR IGNORE in case
                        // a race slipped past the hash check above.
                        {
                            let state = app_handle.state::<AppState>();
                            let session_key = state.session_key.lock().unwrap().clone();
                            if let Some(ref key) = session_key {
                                let db = state.db.lock().unwrap();
                                if let Some(ref conn) = *db {
                                    match parsed.file_id {
                                        Some(ref fid) => {
                                            // File message: store the key encrypted; metadata in columns.
                                            let key_json = serde_json::to_string(
                                                parsed.file_key.as_deref().unwrap_or(&[])
                                            ).unwrap();
                                            let (nonce, ct) = store::encrypt_content(&key_json, key);
                                            if let Some(ref gid) = parsed.gid {
                                                let _ = crate::db::insert_group_received(
                                                    conn,
                                                    gid,
                                                    &sender_id,
                                                    ts,
                                                    &nonce,
                                                    &ct,
                                                    Some(&wire_hash),
                                                    None,
                                                    None,
                                                    None,
                                                    Some(fid.as_str()),
                                                    parsed.file_name.as_deref(),
                                                    parsed.file_mime.as_deref(),
                                                    parsed.file_size,
                                                    parsed.file_name.as_deref(),
                                                    parsed.thumb_data.as_deref(),
                                                );
                                            } else {
                                                let _ = crate::db::insert_received(
                                                    conn,
                                                    &sender_id,
                                                    ts,
                                                    &nonce,
                                                    &ct,
                                                    Some(&wire_hash),
                                                    None,
                                                    None,
                                                    None,
                                                    Some(fid.as_str()),
                                                    parsed.file_name.as_deref(),
                                                    parsed.file_mime.as_deref(),
                                                    parsed.file_size,
                                                    parsed.file_name.as_deref(),
                                                    parsed.thumb_data.as_deref(),
                                                );
                                            }
                                        }
                                        None => {
                                            let (nonce, ct) = store::encrypt_content(&parsed.text, key);
                                            if let Some(ref gid) = parsed.gid {
                                                let _ = crate::db::insert_group_received(
                                                    conn,
                                                    gid,
                                                    &sender_id,
                                                    ts,
                                                    &nonce,
                                                    &ct,
                                                    Some(&wire_hash),
                                                    parsed.r_ts,
                                                    parsed.r_from.as_deref(),
                                                    parsed.r_text.as_deref(),
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                    Some(parsed.text.as_str()), None,
                                                );
                                            } else {
                                                let _ = crate::db::insert_received(
                                                    conn,
                                                    &sender_id,
                                                    ts,
                                                    &nonce,
                                                    &ct,
                                                    Some(&wire_hash),
                                                    parsed.r_ts,
                                                    parsed.r_from.as_deref(),
                                                    parsed.r_text.as_deref(),
                                                    None,
                                                    None,
                                                    None,
                                                    None,
                                                    Some(parsed.text.as_str()), None,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(ref gid) = parsed.gid {
                            app_handle
                                .emit("group_message", json!({
                                    "gid": gid,
                                    "from": sender_id,
                                    "text": parsed.text,
                                    "ts": ts,
                                    "reply_to_ts": parsed.r_ts,
                                    "reply_to_from": parsed.r_from,
                                    "reply_to_text": parsed.r_text,
                                    "file_id": parsed.file_id,
                                    "file_key": parsed.file_key,
                                    "file_name": parsed.file_name,
                                    "file_mime": parsed.file_mime,
                                    "file_size": parsed.file_size,
                                }))
                                .ok();
                        } else {
                            app_handle
                                .emit("message", json!({
                                    "from": sender_id,
                                    "text": parsed.text,
                                    "ts": ts,
                                    "reply_to_ts": parsed.r_ts,
                                    "reply_to_from": parsed.r_from,
                                    "reply_to_text": parsed.r_text,
                                    "file_id": parsed.file_id,
                                    "file_key": parsed.file_key,
                                    "file_name": parsed.file_name,
                                    "file_mime": parsed.file_mime,
                                    "file_size": parsed.file_size,
                                }))
                                .ok();
                        }

                        // System notification — only when the main window is not focused and chat isn't muted.
                        let focused = app_handle
                            .get_webview_window("main")
                            .and_then(|w| w.is_focused().ok())
                            .unwrap_or(false);
                        let muted = {
                            let notif_peer = parsed.gid.as_deref().unwrap_or(&sender_id);
                            let state = app_handle.state::<AppState>();
                            let db = state.db.lock().unwrap();
                            db.as_ref()
                                .map(|c| crate::db::get_mute(c, notif_peer, store::now_ms()).is_muted)
                                .unwrap_or(false)
                        };
                        if !focused && !muted {
                            let state = app_handle.state::<AppState>();
                            let title = if let Some(ref gid) = parsed.gid {
                                state.groups.lock().unwrap()
                                    .get(gid)
                                    .map(|g| g.name.clone())
                                    .unwrap_or_else(|| gid[..8.min(gid.len())].to_string())
                            } else {
                                state.peer_names.lock().unwrap()
                                    .get(&sender_id)
                                    .cloned()
                                    .unwrap_or_else(|| sender_id[..8.min(sender_id.len())].to_string())
                            };
                            let notif_body = if parsed.gid.is_some() {
                                let sender_name = state.peer_names.lock().unwrap()
                                    .get(&sender_id)
                                    .cloned()
                                    .unwrap_or_else(|| sender_id[..8.min(sender_id.len())].to_string());
                                format!("{sender_name}: {}", parsed.text)
                            } else if !parsed.text.is_empty() {
                                parsed.text.clone()
                            } else if let Some(ref name) = parsed.file_name {
                                format!("📎 {name}")
                            } else {
                                "📎 File".to_string()
                            };
                            let _ = app_handle
                                .notification()
                                .builder()
                                .title(&title)
                                .body(&notif_body)
                                .show();
                        }
                        if !encrypted_sessions.is_empty() {
                            if let Some(ref dir) = data_dir {
                                tokio::spawn(store::save_sessions(dir.clone(), encrypted_sessions));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("decrypt error from={sender_id}: {e}");
                        app_handle
                            .emit("msg_error", json!({ "sender": sender_id, "err": e.to_string() }))
                            .ok();
                    }
                }
            }
            Ok(WsMessage::Text(json_str)) => {
                // System events sent by the server as JSON text (presence, hello).
                match serde_json::from_str::<serde_json::Value>(&json_str) {
                    Ok(v) => {
                        let t = v["type"].as_str().unwrap_or("");
                        match t {
                            "presence" => { app_handle.emit("presence", &v).ok(); }
                            "hello"    => { app_handle.emit("hello", &v).ok(); }
                            other      => tracing::warn!("unknown text event type: {other}"),
                        }
                    }
                    Err(e) => tracing::warn!("invalid text frame from server: {e}"),
                }
            }
            Err(e) => {
                tracing::error!("WS stream error: {e}");
                app_handle.emit("connection_lost", ()).ok();
                break;
            }
            _ => {}
        }
    }

    send_task.abort();
}
