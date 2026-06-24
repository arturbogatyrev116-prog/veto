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

#[derive(Deserialize)]
pub struct CreateGroupReq {
    name: String,
    member_ids: Vec<String>,
}

#[derive(Serialize)]
pub struct MemberResp {
    pub user_id: String,
    pub username: String,
}

#[derive(Serialize)]
pub struct GroupResp {
    pub group_id: String,
    pub name: String,
    pub members: Vec<MemberResp>,
}

#[derive(FromRow)]
struct MemberRow {
    user_id: Uuid,
    username: String,
}

async fn fetch_members(state: &AppState, group_id: Uuid) -> Result<Vec<MemberResp>, AppError> {
    let rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT u.id AS user_id, u.username \
         FROM group_members gm JOIN users u ON gm.user_id = u.id \
         WHERE gm.group_id = $1 \
         ORDER BY gm.joined_at",
    )
    .bind(group_id)
    .fetch_all(&state.inner.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| MemberResp { user_id: r.user_id.to_string(), username: r.username })
        .collect())
}

pub async fn create(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CreateGroupReq>,
) -> Result<impl IntoResponse, AppError> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 100 {
        return Err(AppError::BadRequest("group name must be 1–100 chars".into()));
    }
    if req.member_ids.len() > 49 {
        return Err(AppError::BadRequest("max 49 additional members per group".into()));
    }

    let creator_uuid =
        Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("invalid user_id".into()))?;

    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO groups (name, created_by) VALUES ($1, $2) RETURNING group_id",
    )
    .bind(&name)
    .bind(creator_uuid)
    .fetch_one(&state.inner.db)
    .await?;

    sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2)")
        .bind(group_id)
        .bind(creator_uuid)
        .execute(&state.inner.db)
        .await?;

    for member_id_str in &req.member_ids {
        let member_uuid = Uuid::parse_str(member_id_str)
            .map_err(|_| AppError::BadRequest(format!("invalid member_id: {member_id_str}")))?;
        if member_uuid == creator_uuid {
            continue;
        }
        sqlx::query(
            "INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(group_id)
        .bind(member_uuid)
        .execute(&state.inner.db)
        .await?;
    }

    let members = fetch_members(&state, group_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(GroupResp { group_id: group_id.to_string(), name, members }),
    ))
}

pub async fn list(
    auth: AuthUser,
    State(state): State<AppState>,
) -> Result<Json<Vec<GroupResp>>, AppError> {
    let user_uuid =
        Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("invalid user_id".into()))?;

    #[derive(FromRow)]
    struct GroupRow { group_id: Uuid, name: String }

    let rows: Vec<GroupRow> = sqlx::query_as(
        "SELECT g.group_id, g.name \
         FROM groups g \
         JOIN group_members gm ON g.group_id = gm.group_id \
         WHERE gm.user_id = $1 \
         ORDER BY g.created_at DESC",
    )
    .bind(user_uuid)
    .fetch_all(&state.inner.db)
    .await?;

    let mut groups = Vec::with_capacity(rows.len());
    for row in rows {
        let members = fetch_members(&state, row.group_id).await?;
        groups.push(GroupResp { group_id: row.group_id.to_string(), name: row.name, members });
    }

    Ok(Json(groups))
}

pub async fn leave(
    auth: AuthUser,
    Path((group_id_str, member_id_str)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    // Only allow users to remove themselves.
    if auth.user_id != member_id_str {
        return Err(AppError::Forbidden);
    }
    let leaver_uid = &member_id_str;
    let user_uuid = Uuid::parse_str(&auth.user_id)
        .map_err(|_| AppError::BadRequest("invalid user_id".into()))?;
    let group_uuid = Uuid::parse_str(&group_id_str)
        .map_err(|_| AppError::BadRequest("invalid group_id".into()))?;

    // Fetch remaining members BEFORE deleting, so we know who to notify.
    let remaining: Vec<String> = fetch_members(&state, group_uuid).await?
        .into_iter()
        .filter(|m| m.user_id != *leaver_uid)
        .map(|m| m.user_id)
        .collect();

    sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group_uuid)
        .bind(user_uuid)
        .execute(&state.inner.db)
        .await?;

    crate::routes::ws::notify_member_left(&state, &group_id_str, leaver_uid, &remaining);

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_one(
    auth: AuthUser,
    Path(group_id_str): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<GroupResp>, AppError> {
    let user_uuid =
        Uuid::parse_str(&auth.user_id).map_err(|_| AppError::BadRequest("invalid user_id".into()))?;
    let group_uuid =
        Uuid::parse_str(&group_id_str).map_err(|_| AppError::BadRequest("invalid group_id".into()))?;

    // Verify the requesting user is a member.
    let is_member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(group_uuid)
    .bind(user_uuid)
    .fetch_one(&state.inner.db)
    .await?;

    if !is_member {
        return Err(AppError::NotFound);
    }

    let name: String =
        sqlx::query_scalar("SELECT name FROM groups WHERE group_id = $1")
            .bind(group_uuid)
            .fetch_optional(&state.inner.db)
            .await?
            .ok_or(AppError::NotFound)?;

    let members = fetch_members(&state, group_uuid).await?;
    Ok(Json(GroupResp { group_id: group_id_str, name, members }))
}
