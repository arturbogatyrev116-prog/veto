use axum::{extract::State, response::IntoResponse, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub token: String,
    pub refresh_token: String,
    pub expires_at_ms: i64,
}

#[derive(FromRow)]
struct RefreshRow {
    session_id: Uuid,
}

pub async fn handler(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    let refresh_hash: Vec<u8> = Sha256::digest(req.refresh_token.as_bytes()).to_vec();

    let row = sqlx::query_as::<_, RefreshRow>(
        "SELECT t.session_id \
         FROM auth_tokens t \
         JOIN users u ON u.id = t.user_id \
         WHERE t.refresh_token_hash = $1 \
           AND u.blocked = false \
           AND t.refresh_expires_at IS NOT NULL \
           AND t.refresh_expires_at > now()",
    )
    .bind(refresh_hash.as_slice())
    .fetch_optional(&state.inner.db)
    .await
    .map_err(|e| {
        tracing::error!(err = %e, "refresh token lookup failed");
        AppError::Internal
    })?
    .ok_or(AppError::Unauthorized)?;

    // Rotate both tokens.
    let new_token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let new_token_hash: Vec<u8> = Sha256::digest(new_token.as_bytes()).to_vec();
    let new_refresh = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let new_refresh_hash: Vec<u8> = Sha256::digest(new_refresh.as_bytes()).to_vec();

    let expires_at = Utc::now() + chrono::Duration::hours(24);
    let refresh_expires_at = Utc::now() + chrono::Duration::days(30);

    sqlx::query(
        "UPDATE auth_tokens \
         SET token_hash = $1, \
             expires_at = $2, \
             refresh_token_hash = $3, \
             refresh_expires_at = $4, \
             last_seen = now() \
         WHERE session_id = $5",
    )
    .bind(new_token_hash.as_slice())
    .bind(expires_at)
    .bind(new_refresh_hash.as_slice())
    .bind(refresh_expires_at)
    .bind(row.session_id)
    .execute(&state.inner.db)
    .await
    .map_err(|e| {
        tracing::error!(err = %e, "token rotation failed");
        AppError::Internal
    })?;

    tracing::info!(session_id = %row.session_id, "access token refreshed");

    Ok(Json(RefreshResponse {
        token: new_token,
        refresh_token: new_refresh,
        expires_at_ms: expires_at.timestamp_millis(),
    }))
}
