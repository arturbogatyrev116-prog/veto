use std::collections::HashMap;
use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use messenger_crypto::ratchet::RatchetState;

use crate::PeerSession;

// ── Identity ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct StoredIdentity {
    pub user_id: String,
    pub username: String,
    pub token: String,
    pub signing_key: [u8; 32],
    pub spk_secret: [u8; 32],
    pub spk_id: u32,
    /// Unix ms timestamp of the last SPK upload. 0 = never rotated (pre-rotation builds).
    #[serde(default)]
    pub spk_rotation_ts: i64,
    /// One-time prekey private key pool: (opk_id, secret_bytes).
    #[serde(default)]
    pub opk_secrets: Vec<(u32, [u8; 32])>,
    /// Next OPK id to use when generating a replenishment batch.
    #[serde(default)]
    pub opk_next_id: u32,
    /// ML-KEM-768 decapsulation key (2400 bytes). Empty on pre-PQ installations;
    /// populated on registration and refreshed on each SPK rotation.
    #[serde(default)]
    pub pq_spk_secret: Vec<u8>,
}

pub async fn load(path: impl AsRef<Path>) -> Option<StoredIdentity> {
    let bytes = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub async fn save(path: impl AsRef<Path>, identity: &StoredIdentity) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let bytes = serde_json::to_vec_pretty(identity)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    tokio::fs::write(path, bytes).await
}

// ── Data directory ────────────────────────────────────────────────────────────

/// Resolves the app data directory. Honours `MESSENGER_DATA_DIR` so a second
/// instance can be launched with a separate identity:
///   $env:MESSENGER_DATA_DIR = "C:\Temp\user2"; & .\messenger-app.exe
pub fn data_dir(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    if let Ok(dir) = std::env::var("MESSENGER_DATA_DIR") {
        return Ok(std::path::PathBuf::from(dir));
    }
    app.path().app_data_dir().map_err(|e| e.to_string())
}

// ── Session key derivation ────────────────────────────────────────────────────

/// Derive a 32-byte session encryption key from a user password and salt using
/// Argon2id (m=64 MiB, t=3, p=1). Takes ~300-500 ms on a modern desktop CPU.
pub fn derive_session_key(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let params = Params::new(65536, 3, 1, Some(32)).expect("valid argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .expect("argon2 hash");
    key
}

/// Load the sessions.salt file or generate a fresh random 16-byte salt.
pub async fn load_or_create_salt(data_dir: &Path) -> std::io::Result<[u8; 16]> {
    let path = data_dir.join("sessions.salt");
    if let Ok(bytes) = tokio::fs::read(&path).await {
        if bytes.len() == 16 {
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&bytes);
            return Ok(salt);
        }
    }
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    tokio::fs::create_dir_all(data_dir).await?;
    tokio::fs::write(&path, &salt).await?;
    Ok(salt)
}

// ── Session persistence ───────────────────────────────────────────────────────

/// Serialize only `Established` sessions and encrypt them with `key`.
/// Call this while holding the sessions lock; write bytes to disk after releasing.
pub fn extract_and_encrypt(
    sessions: &HashMap<String, PeerSession>,
    key: &[u8; 32],
) -> Vec<u8> {
    let pairs: Vec<(&str, &RatchetState)> = sessions
        .iter()
        .filter_map(|(k, v)| match v {
            PeerSession::Established { ratchet } => Some((k.as_str(), ratchet)),
            _ => None,
        })
        .collect();

    if pairs.is_empty() {
        return Vec::new();
    }

    let plaintext = match postcard::to_stdvec(&pairs) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let Ok(ciphertext) = cipher.encrypt(nonce, plaintext.as_slice()) else {
        return Vec::new();
    };

    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    result
}

/// Write encrypted session bytes to disk atomically (.tmp → rename).
pub async fn save_sessions(
    data_dir: std::path::PathBuf,
    encrypted: Vec<u8>,
) -> std::io::Result<()> {
    if encrypted.is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&data_dir).await?;
    let tmp = data_dir.join("sessions.bin.tmp");
    tokio::fs::write(&tmp, encrypted).await?;
    tokio::fs::rename(tmp, data_dir.join("sessions.bin")).await
}

// ── Per-message encryption ────────────────────────────────────────────────────

/// Encrypt a single message text for storage in the local SQLite DB.
/// Uses a fresh random nonce each call.
pub fn encrypt_content(text: &str, key: &[u8; 32]) -> ([u8; 12], Vec<u8>) {
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let ct = cipher.encrypt(nonce, text.as_bytes()).expect("encrypt content");
    (nonce_bytes, ct)
}

pub fn decrypt_content(nonce: &[u8; 12], ct: &[u8], key: &[u8; 32]) -> Result<String, String> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let pt = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| "content decrypt failed".to_string())?;
    String::from_utf8(pt).map_err(|e| e.to_string())
}

/// Returns a persistent device ID from `{data_dir}/device_id`, generating and
/// saving a new UUID v4 on first call.
pub fn get_or_create_device_id(data_dir: &Path) -> String {
    let path = data_dir.join("device_id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return id;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(data_dir);
    let _ = std::fs::write(&path, &id);
    id
}

/// Milliseconds since Unix epoch — used for message timestamps.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ── Sessions ──────────────────────────────────────────────────────────────────

/// Decrypt and deserialize sessions using the provided key.
///
/// Returns:
/// - `Some(sessions)` — key correct; map may be empty if no sessions were saved yet
/// - `None`           — sessions.bin exists but AEAD tag mismatch → wrong password
pub async fn load_sessions(
    data_dir: &Path,
    key: &[u8; 32],
) -> Option<HashMap<String, PeerSession>> {
    let Ok(bytes) = tokio::fs::read(data_dir.join("sessions.bin")).await else {
        // File doesn't exist — first launch or after clear_identity. Any key is valid.
        return Some(HashMap::new());
    };
    if bytes.len() < 12 {
        return Some(HashMap::new());
    }

    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(&bytes[..12]);
    let Ok(plaintext) = cipher.decrypt(nonce, &bytes[12..]) else {
        return None; // AEAD auth tag mismatch → wrong password
    };

    let Ok(pairs) = postcard::from_bytes::<Vec<(String, RatchetState)>>(&plaintext) else {
        return Some(HashMap::new()); // key correct, but file was corrupted — start fresh
    };

    Some(
        pairs
            .into_iter()
            .map(|(k, ratchet)| (k, PeerSession::Established { ratchet }))
            .collect(),
    )
}
