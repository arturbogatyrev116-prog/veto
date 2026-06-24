use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, state::AppState};

#[derive(FromRow, Serialize)]
pub struct SessionRow {
    pub session_id: Uuid,
    pub device_name: String,
    pub device_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::Unauthorized)?;
    let sessions = sqlx::query_as::<_, SessionRow>(
        "SELECT session_id, device_name, device_id, created_at, last_seen \
         FROM auth_tokens WHERE user_id = $1 ORDER BY last_seen DESC",
    )
    .bind(user_id)
    .fetch_all(&state.inner.db)
    .await
    .map_err(|e| {
        tracing::error!(err = %e, "failed to list sessions");
        AppError::Internal
    })?;

    Ok(Json(sessions))
}

pub async fn revoke(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(session_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::Unauthorized)?;

    let result = sqlx::query(
        "DELETE FROM auth_tokens WHERE session_id = $1 AND user_id = $2",
    )
    .bind(session_id)
    .bind(user_id)
    .execute(&state.inner.db)
    .await
    .map_err(|e| {
        tracing::error!(err = %e, "failed to revoke session");
        AppError::Internal
    })?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
