use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

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

pub async fn by_username(
    Path(username): Path<String>,
    State(state): State<AppState>,
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
