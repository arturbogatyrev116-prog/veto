use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{auth::AuthUser, error::AppError, state::AppState};

type PollRow = (String, String, String, String, serde_json::Value, bool, chrono::DateTime<chrono::Utc>);

#[derive(Deserialize)]
pub struct CreatePollReq {
    pub peer_id: String,
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Deserialize)]
pub struct VoteReq {
    pub option_id: String,
}

#[derive(Serialize, Clone)]
pub struct PollOption {
    pub id: String,
    pub text: String,
    pub votes: i64,
}

#[derive(Serialize)]
pub struct PollResp {
    pub id: String,
    pub peer_id: String,
    pub creator_id: String,
    pub question: String,
    pub options: Vec<PollOption>,
    pub my_vote: Option<String>,
    pub total_votes: i64,
    pub closed: bool,
    pub created_at: String,
}

pub async fn create(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CreatePollReq>,
) -> Result<impl IntoResponse, AppError> {
    if req.question.trim().is_empty() {
        return Err(AppError::BadRequest("question is empty".into()));
    }
    if req.options.len() < 2 || req.options.len() > 10 {
        return Err(AppError::BadRequest("polls require 2-10 options".into()));
    }

    let creator_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user id".into()))?;
    let poll_id = Uuid::new_v4();

    let options_json: Vec<serde_json::Value> = req.options.iter().enumerate().map(|(i, text)| {
        serde_json::json!({ "id": format!("opt-{i}"), "text": text.trim() })
    }).collect();
    let options_json = serde_json::Value::Array(options_json);

    sqlx::query(
        "INSERT INTO polls (id, peer_id, creator_id, question, options) VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(poll_id)
    .bind(&req.peer_id)
    .bind(creator_id)
    .bind(&req.question)
    .bind(&options_json)
    .execute(&state.inner.db)
    .await?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "poll_id": poll_id.to_string() }))))
}

pub async fn get_one(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(poll_id_str): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let poll_id = Uuid::parse_str(&poll_id_str).map_err(|_| AppError::BadRequest("bad poll id".into()))?;
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user id".into()))?;

    build_poll_resp(&state, poll_id, user_id).await.map(Json)
}

pub async fn vote(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(poll_id_str): Path<String>,
    Json(req): Json<VoteReq>,
) -> Result<impl IntoResponse, AppError> {
    let poll_id = Uuid::parse_str(&poll_id_str).map_err(|_| AppError::BadRequest("bad poll id".into()))?;
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user id".into()))?;

    // Verify poll exists and is open
    let row: Option<(bool, serde_json::Value)> = sqlx::query_as(
        "SELECT closed, options FROM polls WHERE id = $1"
    )
    .bind(poll_id)
    .fetch_optional(&state.inner.db)
    .await?;

    let (closed, options) = row.ok_or(AppError::NotFound)?;
    if closed {
        return Err(AppError::BadRequest("poll is closed".into()));
    }

    // Verify option_id is valid
    let valid = options.as_array()
        .map(|arr| arr.iter().any(|o| o["id"].as_str() == Some(&req.option_id)))
        .unwrap_or(false);
    if !valid {
        return Err(AppError::BadRequest("invalid option_id".into()));
    }

    // Upsert vote
    sqlx::query(
        "INSERT INTO poll_votes (poll_id, user_id, option_id) VALUES ($1, $2, $3)
         ON CONFLICT (poll_id, user_id) DO UPDATE SET option_id = $3, voted_at = now()"
    )
    .bind(poll_id)
    .bind(user_id)
    .bind(&req.option_id)
    .execute(&state.inner.db)
    .await?;

    build_poll_resp(&state, poll_id, user_id).await.map(Json)
}

pub async fn close(
    auth: AuthUser,
    State(state): State<AppState>,
    Path(poll_id_str): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let poll_id = Uuid::parse_str(&poll_id_str).map_err(|_| AppError::BadRequest("bad poll id".into()))?;
    let user_id = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user id".into()))?;

    let creator: Option<(Uuid,)> = sqlx::query_as(
        "SELECT creator_id FROM polls WHERE id = $1"
    )
    .bind(poll_id)
    .fetch_optional(&state.inner.db)
    .await?;

    let (creator_id,) = creator.ok_or(AppError::NotFound)?;
    if creator_id != user_id {
        return Err(AppError::Unauthorized);
    }

    sqlx::query("UPDATE polls SET closed = true WHERE id = $1")
        .bind(poll_id)
        .execute(&state.inner.db)
        .await?;

    build_poll_resp(&state, poll_id, user_id).await.map(Json)
}

async fn build_poll_resp(state: &AppState, poll_id: Uuid, user_id: Uuid) -> Result<PollResp, AppError> {
    let row: Option<PollRow> =
        sqlx::query_as(
            "SELECT id::text, peer_id, creator_id::text, question, options, closed, created_at FROM polls WHERE id = $1"
        )
        .bind(poll_id)
        .fetch_optional(&state.inner.db)
        .await?;

    let (id, peer_id, creator_id, question, options_json, closed, created_at) = row.ok_or(AppError::NotFound)?;

    // Fetch vote counts per option
    let vote_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT option_id, COUNT(*) FROM poll_votes WHERE poll_id = $1 GROUP BY option_id"
    )
    .bind(poll_id)
    .fetch_all(&state.inner.db)
    .await?;
    let vote_map: std::collections::HashMap<String, i64> = vote_rows.into_iter().collect();

    // Fetch caller's vote
    let my_vote: Option<(String,)> = sqlx::query_as(
        "SELECT option_id FROM poll_votes WHERE poll_id = $1 AND user_id = $2"
    )
    .bind(poll_id)
    .bind(user_id)
    .fetch_optional(&state.inner.db)
    .await?;

    let options: Vec<PollOption> = options_json.as_array().unwrap_or(&vec![]).iter().map(|o| {
        let opt_id = o["id"].as_str().unwrap_or("").to_string();
        let votes = *vote_map.get(&opt_id).unwrap_or(&0);
        PollOption { id: opt_id, text: o["text"].as_str().unwrap_or("").to_string(), votes }
    }).collect();

    let total_votes: i64 = options.iter().map(|o| o.votes).sum();

    Ok(PollResp {
        id,
        peer_id,
        creator_id,
        question,
        options,
        my_vote: my_vote.map(|(s,)| s),
        total_votes,
        closed,
        created_at: created_at.to_rfc3339(),
    })
}
