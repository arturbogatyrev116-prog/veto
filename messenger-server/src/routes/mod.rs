pub mod admin;
pub mod files;
pub mod groups;
pub mod health;
pub mod prekeys;
pub mod register;
pub mod sessions;
pub mod updates;
pub mod users;
pub mod ws;

use axum::{
    middleware,
    routing::{delete, get, post, put},
    Router,
};


use crate::{auth, state::AppState};

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/health", get(health::handler))
        .route("/api/v1/register", post(register::handler))
        .route("/api/v1/users/{user_id}/prekeys", put(prekeys::upload))
        .route("/api/v1/users/{user_id}/prekeys", get(prekeys::fetch))
        .route("/api/v1/users/{user_id}/opks", post(prekeys::upload_opks))
        .route("/api/v1/users/{user_id}/key-log", get(prekeys::get_key_log))
        .route("/api/v1/users/by-username/{username}", get(users::by_username))
        .route("/api/v1/files", post(files::upload))
        .route("/api/v1/files/{file_id}", get(files::download))
        .route("/api/v1/groups", post(groups::create))
        .route("/api/v1/groups", get(groups::list))
        .route("/api/v1/groups/{group_id}", get(groups::get_one))
        .route("/api/v1/groups/{group_id}/members/{user_id}", delete(groups::leave))
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{session_id}", delete(sessions::revoke))
        .route("/api/v1/updates/{target}/{current_version}", get(updates::check))
        .route("/api/v1/admin/users", get(admin::list_users))
        .route("/api/v1/admin/users", post(admin::create_user))
        .route("/api/v1/admin/users/{user_id}/block", post(admin::block_user))
        .route("/api/v1/admin/users/{user_id}/unblock", post(admin::unblock_user))
        .route("/api/v1/admin/users/{user_id}", delete(admin::delete_user))
        .route("/ws", get(ws::handler))
        .layer(middleware::from_fn_with_state(state, auth::rate_limit))
}
