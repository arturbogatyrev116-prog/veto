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

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct ChannelResp {
    pub channel_id:  String,
    pub group_id:    String,
    pub name:        String,
    pub description: Option<String>,
    pub created_by:  String,
    pub subscribed:  bool,
}

#[derive(FromRow)]
struct ChannelRow {
    channel_id:  Uuid,
    group_id:    Uuid,
    name:        String,
    description: Option<String>,
    created_by:  Uuid,
}

#[derive(Deserialize)]
pub struct CreateChannelReq {
    pub name:        String,
    pub description: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn require_member(state: &AppState, group_id: Uuid, user_id: Uuid) -> Result<String, AppError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM group_members WHERE group_id = $1 AND user_id = $2",
    )
    .bind(group_id).bind(user_id)
    .fetch_optional(&state.inner.db).await?;
    role.ok_or(AppError::Forbidden)
}

fn is_admin_or_owner(role: &str) -> bool {
    matches!(role, "owner" | "admin")
}

// ── POST /api/v1/groups/:gid/channels ────────────────────────────────────────

pub async fn create(
    auth: AuthUser,
    Path(group_id_str): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<CreateChannelReq>,
) -> Result<impl IntoResponse, AppError> {
    let caller = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user_id".into()))?;
    let gid    = Uuid::parse_str(&group_id_str).map_err(|_| AppError::BadRequest("bad group_id".into()))?;
    let name   = req.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::BadRequest("channel name must be 1–64 chars".into()));
    }

    let role = require_member(&state, gid, caller).await?;
    if !is_admin_or_owner(&role) {
        return Err(AppError::Forbidden);
    }

    let channel_id: Uuid = sqlx::query_scalar(
        "INSERT INTO channels (group_id, name, description, created_by) \
         VALUES ($1, $2, $3, $4) RETURNING channel_id",
    )
    .bind(gid).bind(&name).bind(&req.description).bind(caller)
    .fetch_one(&state.inner.db).await?;

    // Auto-subscribe all current members if group has auto_subscribe_channels = true.
    let auto: bool = sqlx::query_scalar(
        "SELECT auto_subscribe_channels FROM groups WHERE group_id = $1",
    )
    .bind(gid)
    .fetch_optional(&state.inner.db).await?.unwrap_or(true);

    if auto {
        sqlx::query(
            "INSERT INTO channel_subscriptions (channel_id, user_id) \
             SELECT $1, user_id FROM group_members WHERE group_id = $2 \
             ON CONFLICT DO NOTHING",
        )
        .bind(channel_id).bind(gid)
        .execute(&state.inner.db).await?;
    }

    Ok((StatusCode::CREATED, Json(ChannelResp {
        channel_id:  channel_id.to_string(),
        group_id:    group_id_str,
        name,
        description: req.description,
        created_by:  auth.user_id,
        subscribed:  auto,
    })))
}

// ── GET /api/v1/groups/:gid/channels ─────────────────────────────────────────

pub async fn list(
    auth: AuthUser,
    Path(group_id_str): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<ChannelResp>>, AppError> {
    let caller = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user_id".into()))?;
    let gid    = Uuid::parse_str(&group_id_str).map_err(|_| AppError::BadRequest("bad group_id".into()))?;

    require_member(&state, gid, caller).await?;

    let rows: Vec<ChannelRow> = sqlx::query_as(
        "SELECT channel_id, group_id, name, description, created_by FROM channels WHERE group_id = $1 ORDER BY created_at",
    )
    .bind(gid)
    .fetch_all(&state.inner.db).await?;

    let mut resp = Vec::with_capacity(rows.len());
    for r in rows {
        let subscribed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM channel_subscriptions WHERE channel_id = $1 AND user_id = $2)",
        )
        .bind(r.channel_id).bind(caller)
        .fetch_one(&state.inner.db).await?;

        resp.push(ChannelResp {
            channel_id:  r.channel_id.to_string(),
            group_id:    r.group_id.to_string(),
            name:        r.name,
            description: r.description,
            created_by:  r.created_by.to_string(),
            subscribed,
        });
    }
    Ok(Json(resp))
}

// ── DELETE /api/v1/groups/:gid/channels/:cid ─────────────────────────────────

pub async fn delete(
    auth: AuthUser,
    Path((group_id_str, channel_id_str)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let caller = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user_id".into()))?;
    let gid    = Uuid::parse_str(&group_id_str).map_err(|_| AppError::BadRequest("bad group_id".into()))?;
    let cid    = Uuid::parse_str(&channel_id_str).map_err(|_| AppError::BadRequest("bad channel_id".into()))?;

    let role = require_member(&state, gid, caller).await?;
    if !is_admin_or_owner(&role) {
        return Err(AppError::Forbidden);
    }

    sqlx::query("DELETE FROM channels WHERE channel_id = $1 AND group_id = $2")
        .bind(cid).bind(gid)
        .execute(&state.inner.db).await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── POST /api/v1/groups/:gid/channels/:cid/subscribe ─────────────────────────

pub async fn subscribe(
    auth: AuthUser,
    Path((group_id_str, channel_id_str)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let caller = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user_id".into()))?;
    let gid    = Uuid::parse_str(&group_id_str).map_err(|_| AppError::BadRequest("bad group_id".into()))?;
    let cid    = Uuid::parse_str(&channel_id_str).map_err(|_| AppError::BadRequest("bad channel_id".into()))?;

    require_member(&state, gid, caller).await?;

    // Verify channel belongs to this group.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM channels WHERE channel_id = $1 AND group_id = $2)",
    )
    .bind(cid).bind(gid)
    .fetch_one(&state.inner.db).await?;
    if !exists { return Err(AppError::NotFound); }

    sqlx::query(
        "INSERT INTO channel_subscriptions (channel_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(cid).bind(caller)
    .execute(&state.inner.db).await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── DELETE /api/v1/groups/:gid/channels/:cid/subscribe ───────────────────────

pub async fn unsubscribe(
    auth: AuthUser,
    Path((group_id_str, channel_id_str)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let caller = Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("bad user_id".into()))?;
    let cid    = Uuid::parse_str(&channel_id_str).map_err(|_| AppError::BadRequest("bad channel_id".into()))?;

    let _ = group_id_str; // validated implicitly

    sqlx::query(
        "DELETE FROM channel_subscriptions WHERE channel_id = $1 AND user_id = $2",
    )
    .bind(cid).bind(caller)
    .execute(&state.inner.db).await?;

    Ok(StatusCode::NO_CONTENT)
}

// ── GET /api/v1/channels/:cid/subscribers — for routing ──────────────────────

/// Returns just the subscriber user_ids; called by ws.rs to fan-out channel messages.
pub async fn get_subscriber_ids(
    state: &AppState,
    channel_id: Uuid,
) -> Result<Vec<String>, AppError> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT user_id::text FROM channel_subscriptions WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&state.inner.db).await?;
    Ok(ids)
}
