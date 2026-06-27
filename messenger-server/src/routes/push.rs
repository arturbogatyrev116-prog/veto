use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;

use crate::{auth::AuthUser, state::AppState};

#[derive(Deserialize)]
pub struct TokenBody {
    pub platform: String,
    pub token: String,
}

pub async fn register(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<TokenBody>,
) -> StatusCode {
    if !matches!(body.platform.as_str(), "fcm" | "apns") {
        return StatusCode::BAD_REQUEST;
    }
    if body.token.is_empty() || body.token.len() > 4096 {
        return StatusCode::BAD_REQUEST;
    }

    let Ok(uid) = uuid::Uuid::parse_str(&user.user_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR;
    };

    let res = sqlx::query(
        "INSERT INTO push_tokens (user_id, session_id, platform, token)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, token) DO UPDATE
             SET session_id = $2, platform = $3, created_at = now()",
    )
    .bind(uid)
    .bind(user.session_id)
    .bind(&body.platform)
    .bind(&body.token)
    .execute(&state.inner.db)
    .await;

    match res {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            tracing::error!("push token register error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub async fn unregister(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<TokenBody>,
) -> StatusCode {
    let Ok(uid) = uuid::Uuid::parse_str(&user.user_id) else {
        return StatusCode::INTERNAL_SERVER_ERROR;
    };

    let _ = sqlx::query(
        "DELETE FROM push_tokens WHERE user_id = $1 AND token = $2",
    )
    .bind(uid)
    .bind(&body.token)
    .execute(&state.inner.db)
    .await;

    StatusCode::NO_CONTENT
}
