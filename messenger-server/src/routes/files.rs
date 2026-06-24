use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use sha2::{Digest, Sha256};
use serde_json::json;
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, state::AppState};

const MAX_FILE_SIZE: usize = 20 * 1024 * 1024; // 20 MB

/// Upload an E2E-encrypted file blob.
///
/// The client encrypts the file before sending — this endpoint stores an opaque
/// blob and returns a UUID that the sender includes in the ratchet message payload.
///
/// POST /api/v1/files  (Bearer auth required)
/// Body: raw encrypted bytes (max 20 MB)
/// Response: { "file_id": "<uuid>" }
pub async fn upload(
    auth: AuthUser,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    if body.is_empty() {
        return Err(AppError::BadRequest("empty file body".into()));
    }
    if body.len() > MAX_FILE_SIZE {
        return Err(AppError::BadRequest("file too large (max 20 MB)".into()));
    }

    let uploader_uuid = Uuid::parse_str(&auth.user_id)
        .map_err(|_| AppError::BadRequest("invalid user_id in token".into()))?;

    let hash: Vec<u8> = Sha256::digest(&body).to_vec();
    let size = body.len() as i64;

    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO file_store (uploader_id, data, size_bytes, hash)
         VALUES ($1, $2, $3, $4)
         RETURNING file_id",
    )
    .bind(uploader_uuid)
    .bind(body.as_ref())
    .bind(size)
    .bind(&hash)
    .fetch_one(&state.inner.db)
    .await?;

    Ok((StatusCode::CREATED, Json(json!({ "file_id": row.0.to_string() }))))
}

/// Download an E2E-encrypted file blob by ID.
///
/// Any authenticated user can fetch any file by UUID (the blob is encrypted —
/// the server cannot distinguish who should access it).
///
/// GET /api/v1/files/{file_id}  (Bearer auth required)
/// Response: raw encrypted bytes as application/octet-stream
pub async fn download(
    _auth: AuthUser,
    Path(file_id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let uuid = Uuid::parse_str(&file_id)
        .map_err(|_| AppError::BadRequest("invalid file_id".into()))?;

    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT data FROM file_store WHERE file_id = $1")
            .bind(uuid)
            .fetch_optional(&state.inner.db)
            .await?;

    let (data,) = row.ok_or(AppError::NotFound)?;

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        data,
    ))
}
