use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    #[serde(default = "default_device_id")]
    pub device_id: String,
}

fn default_device_name() -> String { "Unknown Device".to_string() }
fn default_device_id() -> String { Uuid::new_v4().to_string() }

#[derive(Serialize)]
pub struct RegisterResponse {
    pub user_id: String,
    pub token: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    if !state.inner.registration_open {
        return Err(AppError::Forbidden);
    }

    let username = req.username.trim().to_owned();
    if username.is_empty() || username.len() > 64 {
        return Err(AppError::BadRequest("username must be 1–64 characters".into()));
    }

    let user_id = Uuid::new_v4();
    // Two UUIDs concatenated: 244 random bits of entropy.
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();

    sqlx::query("INSERT INTO users (id, username) VALUES ($1, $2)")
        .bind(user_id)
        .bind(&username)
        .execute(&state.inner.db)
        .await
        .map_err(|e| {
            if let sqlx::Error::Database(ref db_err) = e {
                if db_err.constraint() == Some("users_username_key") {
                    return AppError::BadRequest("username already taken".into());
                }
            }
            tracing::error!(err = %e, "failed to insert user");
            AppError::Internal
        })?;

    let device_name = req.device_name.trim().chars().take(128).collect::<String>();
    let device_id = req.device_id.trim().chars().take(128).collect::<String>();

    // Refuse registration from a blocked device.
    let device_blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM blocked_devices WHERE device_id = $1)",
    )
    .bind(&device_id)
    .fetch_one(&state.inner.db)
    .await?;

    if device_blocked {
        // Roll back the user row we just inserted.
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&state.inner.db)
            .await;
        return Err(AppError::Forbidden);
    }

    sqlx::query(
        "INSERT INTO auth_tokens (token_hash, user_id, device_name, device_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(token_hash.as_slice())
    .bind(user_id)
    .bind(&device_name)
    .bind(&device_id)
    .execute(&state.inner.db)
    .await?;

    tracing::info!(%user_id, username, "user registered");

    Ok((
        StatusCode::CREATED,
        Json(RegisterResponse {
            user_id: user_id.to_string(),
            token,
        }),
    ))
}
