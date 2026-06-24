use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use messenger_crypto::keys::{self, PreKeyBundle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;
use x25519_dalek::PublicKey as X25519Public;

use crate::{auth::AuthUser, error::AppError, state::AppState};

pub async fn upload(
    Path(user_id): Path<String>,
    auth: AuthUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    if auth.user_id != user_id {
        return Err(AppError::Forbidden);
    }
    if body.is_empty() {
        return Err(AppError::BadRequest("empty prekey bundle".into()));
    }
    if body.len() > 256 * 1024 {
        return Err(AppError::BadRequest("prekey bundle too large (max 256 KiB)".into()));
    }

    let user_uuid = Uuid::parse_str(&user_id)
        .map_err(|_| AppError::BadRequest("invalid user_id".into()))?;

    // Parse early to extract canonical key bytes for the transparency log.
    // Using canonical form (IK || SPK || PQ-SPK) rather than the raw body so the
    // hash is stable: the fetch endpoint patches in an OPK that would change the body.
    let bundle = keys::bundle_from_bytes(&body)
        .map_err(|_| AppError::BadRequest("invalid prekey bundle encoding".into()))?;
    let mut bundle_canonical: Vec<u8> = Vec::with_capacity(32 + 32 + 1184);
    bundle_canonical.extend_from_slice(bundle.identity_key.as_bytes());
    bundle_canonical.extend_from_slice(bundle.signed_prekey.as_bytes());
    if let Some(ref pq) = bundle.pq_spk_public {
        bundle_canonical.extend_from_slice(pq);
    }

    sqlx::query(
        "INSERT INTO prekey_bundles (user_id, bundle_bytes)
         VALUES ($1, $2)
         ON CONFLICT (user_id)
         DO UPDATE SET bundle_bytes = EXCLUDED.bundle_bytes, updated_at = now()",
    )
    .bind(user_uuid)
    .bind(body.as_ref())
    .execute(&state.inner.db)
    .await?;

    append_key_log(&state, user_uuid, "static_bundle", &bundle_canonical).await?;
    tracing::debug!(%user_id, bytes = body.len(), "prekey bundle stored");
    Ok(StatusCode::NO_CONTENT)
}

/// Append one entry to the per-user key transparency log.
/// Uses a serializing transaction (SELECT ... FOR UPDATE) to prevent concurrent
/// inserts from racing on the same prev_hash.
async fn append_key_log(
    state: &AppState,
    user_id: Uuid,
    key_type: &str,
    key_bytes: &[u8],
) -> Result<(), AppError> {
    let mut tx = state.inner.db.begin().await?;

    let prev: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT entry_hash FROM key_log WHERE user_id=$1 ORDER BY id DESC LIMIT 1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let key_hash = Sha256::digest(key_bytes).to_vec();
    let prev_for_hash = prev.clone().unwrap_or_else(|| vec![0u8; 32]);
    let unix_ts: i64 = chrono::Utc::now().timestamp();

    let mut buf = Vec::with_capacity(32 + 16 + 4 + key_type.len() + 32 + 8);
    buf.extend_from_slice(&prev_for_hash);
    buf.extend_from_slice(user_id.as_bytes());
    buf.extend_from_slice(&(key_type.len() as u32).to_le_bytes());
    buf.extend_from_slice(key_type.as_bytes());
    buf.extend_from_slice(&key_hash);
    buf.extend_from_slice(&unix_ts.to_le_bytes());
    let entry_hash = Sha256::digest(&buf).to_vec();

    sqlx::query(
        "INSERT INTO key_log (user_id, key_type, key_hash, prev_hash, entry_hash) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(key_type)
    .bind(&key_hash)
    .bind(&prev)
    .bind(&entry_hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Upload a batch of one-time prekey (OPK) public keys.
///
/// Body: postcard-encoded `Vec<(u32, [u8; 32])>` — (opk_id, public_key_bytes) pairs.
/// Max 100 OPKs per call. Requires auth token of the owning user.
pub async fn upload_opks(
    Path(user_id): Path<String>,
    auth: AuthUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    if auth.user_id != user_id {
        return Err(AppError::Forbidden);
    }
    if body.is_empty() {
        return Err(AppError::BadRequest("empty OPK batch".into()));
    }
    if body.len() > 16 * 1024 {
        return Err(AppError::BadRequest("OPK batch too large (max 16 KiB)".into()));
    }

    let user_uuid = Uuid::parse_str(&user_id)
        .map_err(|_| AppError::BadRequest("invalid user_id".into()))?;

    let opks: Vec<(u32, [u8; 32])> = postcard::from_bytes(&body)
        .map_err(|_| AppError::BadRequest("invalid OPK batch encoding".into()))?;

    if opks.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    if opks.len() > 100 {
        return Err(AppError::BadRequest("too many OPKs per batch (max 100)".into()));
    }

    for (opk_id, pub_key) in &opks {
        sqlx::query(
            "INSERT INTO opk_pool (user_id, opk_id, public_key) VALUES ($1, $2, $3)",
        )
        .bind(user_uuid)
        .bind(*opk_id as i32)
        .bind(pub_key.as_ref())
        .execute(&state.inner.db)
        .await?;
    }

    tracing::debug!(%user_id, count = opks.len(), "OPK batch stored");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromRow)]
struct BundleRow {
    bundle_bytes: Vec<u8>,
}

#[derive(FromRow)]
struct OpkRow {
    opk_id: i32,
    public_key: Vec<u8>,
}

/// Fetch the prekey bundle for `user_id`, atomically consuming one OPK from the
/// pool (if any) and patching it into the returned bundle.
pub async fn fetch(
    Path(user_id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let user_uuid = Uuid::parse_str(&user_id).map_err(|_| AppError::NotFound)?;

    let row = sqlx::query_as::<_, BundleRow>(
        "SELECT bundle_bytes FROM prekey_bundles WHERE user_id = $1",
    )
    .bind(user_uuid)
    .fetch_optional(&state.inner.db)
    .await?;

    let row = match row {
        Some(r) => r,
        None => return Err(AppError::NotFound),
    };

    // Atomically pop the oldest OPK from this user's pool (SKIP LOCKED so concurrent
    // fetches can't race and hand out the same OPK twice).
    let opk_row = sqlx::query_as::<_, OpkRow>(
        "DELETE FROM opk_pool \
         WHERE id = (SELECT id FROM opk_pool WHERE user_id = $1 ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) \
         RETURNING opk_id, public_key",
    )
    .bind(user_uuid)
    .fetch_optional(&state.inner.db)
    .await?;

    if let Some(opk) = opk_row {
        let pub_key_bytes: [u8; 32] = opk.public_key.try_into().map_err(|_| {
            tracing::error!("OPK public key has wrong length in DB");
            AppError::Internal
        })?;

        let mut bundle: PreKeyBundle = keys::bundle_from_bytes(&row.bundle_bytes).map_err(|e| {
            tracing::error!("failed to deserialize stored bundle: {e}");
            AppError::Internal
        })?;
        bundle.one_time_prekey = Some(X25519Public::from(pub_key_bytes));
        bundle.one_time_prekey_id = Some(opk.opk_id as u32);

        let patched = postcard::to_stdvec(&bundle).map_err(|e| {
            tracing::error!("failed to re-serialize patched bundle: {e}");
            AppError::Internal
        })?;

        Ok((StatusCode::OK, patched).into_response())
    } else {
        Ok((StatusCode::OK, row.bundle_bytes).into_response())
    }
}

#[derive(Deserialize)]
pub struct KeyLogQuery {
    pub since_id: Option<i64>,
}

#[derive(Serialize)]
pub struct KeyLogEntry {
    pub id: i64,
    pub key_type: String,
    pub key_hash: String,
    pub prev_hash: Option<String>,
    pub entry_hash: String,
    pub created_at: i64,
}

#[derive(FromRow)]
struct KeyLogRow {
    id: i64,
    key_type: String,
    key_hash: Vec<u8>,
    prev_hash: Option<Vec<u8>>,
    entry_hash: Vec<u8>,
    created_at: i64,
}

/// Fetch the key transparency log for `user_id` with optional pagination.
/// Query param: `since_id` — only return entries with id > since_id (default 0).
/// Returns at most 200 entries per call, ordered by id ASC.
pub async fn get_key_log(
    Path(user_id): Path<String>,
    Query(params): Query<KeyLogQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<KeyLogEntry>>, AppError> {
    let user_uuid = Uuid::parse_str(&user_id).map_err(|_| AppError::NotFound)?;
    let since_id = params.since_id.unwrap_or(0);

    let rows: Vec<KeyLogRow> = sqlx::query_as(
        "SELECT id, key_type, key_hash, prev_hash, entry_hash, \
         EXTRACT(EPOCH FROM created_at)::bigint AS created_at \
         FROM key_log \
         WHERE user_id = $1 AND id > $2 \
         ORDER BY id ASC \
         LIMIT 200",
    )
    .bind(user_uuid)
    .bind(since_id)
    .fetch_all(&state.inner.db)
    .await?;

    let entries: Vec<KeyLogEntry> = rows
        .into_iter()
        .map(|r| KeyLogEntry {
            id: r.id,
            key_type: r.key_type,
            key_hash: hex::encode(&r.key_hash),
            prev_hash: r.prev_hash.as_deref().map(hex::encode),
            entry_hash: hex::encode(&r.entry_hash),
            created_at: r.created_at,
        })
        .collect();

    Ok(Json(entries))
}
