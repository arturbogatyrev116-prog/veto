use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

/// Client → server Sender Key group broadcast frame.
/// Format: [GROUP_MAGIC: 4B][4B gid_len][gid UTF-8][payload]
pub const GROUP_MAGIC: &[u8] = &[0xCA, 0xFE, 0x04, 0x00];

/// Client → server group read receipt frame.
/// Format: [GROUP_READ_MAGIC: 4B][4B gid_len][gid UTF-8][8B ts_ms BE i64]
/// Server routes the same way as GROUP_MAGIC (fan-out to all group members).
pub const GROUP_READ_MAGIC: &[u8] = &[0xCA, 0xFE, 0x05, 0x00];

/// Server → client notification that a group member has left.
/// Format: [GROUP_MEMBER_LEFT_MAGIC: 4B][u32_BE gid_len][gid UTF-8][u32_BE uid_len][leaver_uid UTF-8]
pub const GROUP_MEMBER_LEFT_MAGIC: &[u8] = &[0xCA, 0xFE, 0x06, 0x00];

/// Broadcast a member-left notification to all remaining group members.
pub fn notify_member_left(state: &AppState, gid: &str, leaver_id: &str, remaining: &[String]) {
    if remaining.is_empty() {
        return;
    }
    let gid_bytes = gid.as_bytes();
    let uid_bytes = leaver_id.as_bytes();
    let mut frame = Vec::with_capacity(4 + 4 + gid_bytes.len() + 4 + uid_bytes.len());
    frame.extend_from_slice(GROUP_MEMBER_LEFT_MAGIC);
    frame.extend_from_slice(&(gid_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(gid_bytes);
    frame.extend_from_slice(&(uid_bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(uid_bytes);

    for member_id in remaining {
        if let Some(tx) = state.inner.sessions.get(member_id) {
            if tx.send(Message::Binary(frame.clone().into())).is_err() {
                drop(tx);
                enqueue_offline(state, member_id, frame.clone());
            }
        } else {
            enqueue_offline(state, member_id, frame.clone());
        }
    }
}

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: String,
}

#[derive(FromRow)]
struct TokenRow {
    user_id: Uuid,
}

pub async fn handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let token_hash: Vec<u8> = Sha256::digest(q.token.as_bytes()).to_vec();

    let row = sqlx::query_as::<_, TokenRow>(
        "SELECT user_id FROM auth_tokens WHERE token_hash = $1",
    )
    .bind(token_hash.as_slice())
    .fetch_optional(&state.inner.db)
    .await
    .map_err(|_| AppError::Unauthorized)?;

    let user_id = row.ok_or(AppError::Unauthorized)?.user_id.to_string();

    Ok(ws.on_upgrade(move |socket| handle(socket, user_id, state)))
}

async fn handle(socket: WebSocket, user_id: String, state: AppState) {
    let (tx, rx) = mpsc::unbounded_channel::<Message>();
    state.inner.sessions.insert(user_id.clone(), tx);
    tracing::info!(user_id, "client connected");

    // Tell the joining user who is already online.
    let online_users: Vec<String> = state
        .inner
        .sessions
        .iter()
        .filter(|e| e.key() != &user_id)
        .map(|e| e.key().clone())
        .collect();
    let hello = serde_json::json!({ "type": "hello", "online_users": online_users }).to_string();
    if let Some(self_tx) = state.inner.sessions.get(&user_id) {
        let _ = self_tx.send(Message::Text(hello.into()));
    }

    // Announce this user's arrival to everyone else.
    let joined = serde_json::json!({ "type": "presence", "user_id": user_id, "online": true })
        .to_string();
    for entry in state.inner.sessions.iter() {
        if entry.key() != &user_id {
            let _ = entry.send(Message::Text(joined.clone().into()));
        }
    }

    let pending = crate::nats::drain_pending(&state.inner.js, &user_id).await;

    let (mut sink, mut stream) = socket.split();

    let send_task = tokio::spawn(async move {
        for payload in pending {
            if sink.send(Message::Binary(payload.into())).await.is_err() {
                return;
            }
        }
        let mut rx = rx;
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    while let Some(result) = stream.next().await {
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(user_id, err = %e, "WebSocket recv error");
                break;
            }
        };
        match msg {
            Message::Binary(data) => {
                if data.len() >= 4 && (&data[..4] == GROUP_MAGIC || &data[..4] == GROUP_READ_MAGIC) {
                    route_group_message(&state, &data, &user_id).await;
                } else {
                    route_message(&state, &data, &user_id);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    state.inner.sessions.remove(&user_id);
    send_task.abort();
    tracing::info!(user_id, "client disconnected");

    // Announce departure to remaining connected users.
    let left = serde_json::json!({ "type": "presence", "user_id": user_id, "online": false })
        .to_string();
    for entry in state.inner.sessions.iter() {
        let _ = entry.send(Message::Text(left.clone().into()));
    }
}

/// Handle `[GROUP_MAGIC][4B gid_len][gid][payload]` frames.
/// Looks up group members in the DB and broadcasts
/// `[4B sender_len][sender_id][GROUP_MAGIC][4B gid_len][gid][payload]` to each one.
async fn route_group_message(state: &AppState, frame: &[u8], sender_id: &str) {
    // Parse [GROUP_MAGIC:4][4B gid_len][gid][payload]
    let after_magic = &frame[4..];
    if after_magic.len() < 4 {
        tracing::warn!("group broadcast frame too short");
        return;
    }
    let gid_len = u32::from_be_bytes(after_magic[..4].try_into().unwrap()) as usize;
    if after_magic.len() < 4 + gid_len {
        tracing::warn!("group broadcast frame: gid truncated");
        return;
    }
    let gid = match std::str::from_utf8(&after_magic[4..4 + gid_len]) {
        Ok(s) => s,
        Err(_) => { tracing::warn!("group broadcast: gid not UTF-8"); return; }
    };
    let gid_uuid = match uuid::Uuid::parse_str(gid) {
        Ok(u) => u,
        Err(_) => { tracing::warn!("group broadcast: gid not a UUID"); return; }
    };

    // Fetch group members from DB.
    let members: Vec<String> = sqlx::query_scalar(
        "SELECT user_id::text FROM group_members WHERE group_id = $1",
    )
    .bind(gid_uuid)
    .fetch_all(&state.inner.db)
    .await
    .unwrap_or_default();

    if members.is_empty() {
        tracing::warn!(gid, "group broadcast: no members or group not found");
        return;
    }

    // Build outgoing payload: [4B sender_len][sender_id][frame_as_received]
    let sid = sender_id.as_bytes();
    let mut payload = Vec::with_capacity(4 + sid.len() + frame.len());
    payload.extend_from_slice(&(sid.len() as u32).to_be_bytes());
    payload.extend_from_slice(sid);
    payload.extend_from_slice(frame);   // includes GROUP_MAGIC + gid + payload

    for member_id in &members {
        if member_id == sender_id {
            continue; // Don't echo back to sender
        }
        if let Some(tx) = state.inner.sessions.get(member_id) {
            if tx.send(Message::Binary(payload.clone().into())).is_err() {
                drop(tx);
                enqueue_offline(state, member_id, payload.clone());
            }
        } else {
            enqueue_offline(state, member_id, payload.clone());
        }
    }
    tracing::info!(gid, sender_id, members = members.len(), "group broadcast");
}

fn route_message(state: &AppState, frame: &[u8], sender_id: &str) {
    // Frame layout: [4B rid_len][rid][4B msg_id][wire_frame]
    // sender_id is taken from the authenticated WS session — NOT from the frame.
    if frame.len() < 4 {
        tracing::warn!("frame too short, dropping");
        return;
    }
    let id_len = u32::from_be_bytes(frame[..4].try_into().expect("4 bytes")) as usize;
    let header_end = 4 + id_len;
    if frame.len() < header_end + 4 {
        tracing::warn!("malformed routing header, dropping");
        return;
    }
    let recipient_id = match std::str::from_utf8(&frame[4..header_end]) {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!("recipient_id not UTF-8, dropping");
            return;
        }
    };
    let msg_id = u32::from_be_bytes(frame[header_end..header_end + 4].try_into().unwrap());
    let wire_frame = &frame[header_end + 4..];

    // Prepend the server-verified sender_id so the recipient can trust it.
    let sid = sender_id.as_bytes();
    let mut payload = Vec::with_capacity(4 + sid.len() + wire_frame.len());
    payload.extend_from_slice(&(sid.len() as u32).to_be_bytes());
    payload.extend_from_slice(sid);
    payload.extend_from_slice(wire_frame);

    if let Some(tx) = state.inner.sessions.get(recipient_id) {
        tracing::info!(recipient_id, "routing message to online recipient");
        if tx.send(Message::Binary(payload.clone().into())).is_err() {
            drop(tx);
            tracing::warn!(recipient_id, "recipient channel closed, queuing");
            enqueue_offline(state, recipient_id, payload);
        } else if msg_id != 0 {
            // Send delivery ACK to sender: [ACK_MAGIC: 4B][msg_id: 4B BE]
            let mut ack = vec![0xCA, 0xFE, 0x02, 0x00];
            ack.extend_from_slice(&msg_id.to_be_bytes());
            if let Some(sender_tx) = state.inner.sessions.get(sender_id) {
                let _ = sender_tx.send(Message::Binary(ack.into()));
            }
        }
    } else {
        enqueue_offline(state, recipient_id, payload);
        tracing::info!(recipient_id, "recipient offline, message queued");
    }
}

fn enqueue_offline(state: &AppState, recipient_id: &str, payload: Vec<u8>) {
    let js = state.inner.js.clone();
    let recipient_id = recipient_id.to_string();
    tokio::spawn(async move {
        crate::nats::publish(&js, &recipient_id, payload).await;
    });
}
