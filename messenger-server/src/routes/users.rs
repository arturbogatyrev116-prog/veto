use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, state::AppState};

#[derive(Serialize)]
pub struct UserLookupResponse {
    pub user_id: String,
    pub username: String,
}

#[derive(FromRow)]
struct UserRow {
    id: Uuid,
    username: String,
}

#[derive(Deserialize)]
pub struct PatchMeBody {
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
}

pub async fn patch_me(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<PatchMeBody>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::Unauthorized)?;

    let display_name = body.display_name.as_deref().map(|s| s.trim().to_string());
    if let Some(ref dn) = display_name {
        if dn.is_empty() || dn.len() > 64 {
            return Err(AppError::BadRequest("display_name must be 1–64 chars".into()));
        }
    }

    let row = sqlx::query_as::<_, UserRow>(
        "UPDATE users SET display_name = $1 WHERE id = $2 RETURNING id, username",
    )
    .bind(display_name.as_deref())
    .bind(user_id)
    .fetch_one(&state.inner.db)
    .await?;

    Ok(Json(MeResponse {
        user_id: row.id.to_string(),
        username: row.username,
        display_name,
    }))
}

pub async fn by_username(
    Path(username): Path<String>,
    State(state): State<AppState>,
    _auth: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT id, username FROM users WHERE username = $1",
    )
    .bind(&username)
    .fetch_optional(&state.inner.db)
    .await?;

    match row {
        Some(r) => Ok((
            StatusCode::OK,
            Json(UserLookupResponse {
                user_id: r.id.to_string(),
                username: r.username,
            }),
        )),
        None => Err(AppError::NotFound),
    }
}
