use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{auth::AdminAuth, error::AppError, state::AppState};

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct AdminUserView {
    pub user_id: String,
    pub username: String,
    pub blocked: bool,
    pub blocked_reason: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_seen: Option<chrono::DateTime<chrono::Utc>>,
    pub session_count: i64,
}

#[derive(Deserialize)]
pub struct CreateUserBody {
    pub username: String,
}

#[derive(Serialize)]
pub struct CreateUserResponse {
    pub user_id: String,
    pub username: String,
    /// Bearer token shown only once — give it to the user.
    pub token: String,
}

#[derive(Deserialize)]
pub struct BlockUserBody {
    pub reason: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /api/v1/admin/users — list all users with session counts.
pub async fn list_users(
    _auth: AdminAuth,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(FromRow)]
    struct Row {
        id: Uuid,
        username: String,
        blocked: bool,
        blocked_reason: Option<String>,
        created_at: chrono::DateTime<chrono::Utc>,
        last_seen: Option<chrono::DateTime<chrono::Utc>>,
        session_count: i64,
    }

    let rows = sqlx::query_as::<_, Row>(
        "SELECT u.id, u.username, u.blocked, u.blocked_reason, u.created_at, \
                MAX(t.last_seen) AS last_seen, \
                COUNT(t.token_hash) AS session_count \
         FROM users u \
         LEFT JOIN auth_tokens t ON t.user_id = u.id \
         GROUP BY u.id \
         ORDER BY u.created_at DESC",
    )
    .fetch_all(&state.inner.db)
    .await?;

    let users: Vec<AdminUserView> = rows
        .into_iter()
        .map(|r| AdminUserView {
            user_id: r.id.to_string(),
            username: r.username,
            blocked: r.blocked,
            blocked_reason: r.blocked_reason,
            created_at: r.created_at,
            last_seen: r.last_seen,
            session_count: r.session_count,
        })
        .collect();

    Ok(Json(users))
}

/// POST /api/v1/admin/users — create a user and return a bearer token.
pub async fn create_user(
    _auth: AdminAuth,
    State(state): State<AppState>,
    Json(body): Json<CreateUserBody>,
) -> Result<impl IntoResponse, AppError> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};

    let username = body.username.trim().to_string();
    if username.is_empty() || username.len() > 64 {
        return Err(AppError::BadRequest("username must be 1–64 characters".into()));
    }

    let user_id: Uuid = sqlx::query_scalar("INSERT INTO users (username) VALUES ($1) RETURNING id")
        .bind(&username)
        .fetch_one(&state.inner.db)
        .await
        .map_err(|e| {
            if e.to_string().contains("unique") {
                AppError::BadRequest("username already taken".into())
            } else {
                tracing::error!(err = %e, "create_user db error");
                AppError::Internal
            }
        })?;

    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = hex::encode(raw);
    let token_hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();

    sqlx::query("INSERT INTO auth_tokens (token_hash, user_id) VALUES ($1, $2)")
        .bind(token_hash.as_slice())
        .bind(user_id)
        .execute(&state.inner.db)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(CreateUserResponse {
            user_id: user_id.to_string(),
            username,
            token,
        }),
    ))
}

/// POST /api/v1/admin/users/{user_id}/block
/// Blocks user account + all device_ids associated with their sessions.
pub async fn block_user(
    _auth: AdminAuth,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
    body: Option<Json<BlockUserBody>>,
) -> Result<impl IntoResponse, AppError> {
    let reason = body.and_then(|b| b.0.reason);

    let rows = sqlx::query(
        "UPDATE users SET blocked = true, blocked_at = now(), blocked_reason = $1 WHERE id = $2",
    )
    .bind(&reason)
    .bind(user_id)
    .execute(&state.inner.db)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::NotFound);
    }

    // Block all device_ids associated with this user's sessions.
    sqlx::query(
        "INSERT INTO blocked_devices (device_id, reason) \
         SELECT DISTINCT device_id, $1 \
         FROM auth_tokens \
         WHERE user_id = $2 AND device_id IS NOT NULL \
         ON CONFLICT (device_id) DO NOTHING",
    )
    .bind(&reason)
    .bind(user_id)
    .execute(&state.inner.db)
    .await?;

    // Disconnect live session immediately.
    state.inner.sessions.remove(&user_id.to_string());

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/admin/users/{user_id}/unblock
pub async fn unblock_user(
    _auth: AdminAuth,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query(
        "UPDATE users SET blocked = false, blocked_at = NULL, blocked_reason = NULL WHERE id = $1",
    )
    .bind(user_id)
    .execute(&state.inner.db)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::NotFound);
    }

    // Remove device blocks for this user's sessions.
    sqlx::query(
        "DELETE FROM blocked_devices \
         WHERE device_id IN (SELECT DISTINCT device_id FROM auth_tokens WHERE user_id = $1)",
    )
    .bind(user_id)
    .execute(&state.inner.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/admin/users/{user_id} — permanently delete a user and all their data.
pub async fn delete_user(
    _auth: AdminAuth,
    Path(user_id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    state.inner.sessions.remove(&user_id.to_string());

    let rows = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&state.inner.db)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
