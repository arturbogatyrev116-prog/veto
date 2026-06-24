use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{request::Parts, HeaderMap},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

// ── AuthUser extractor ────────────────────────────────────────────────────────

pub struct AuthUser {
    pub user_id: String,
    pub session_id: String,
}

#[derive(FromRow)]
struct TokenRow {
    user_id: Uuid,
    session_id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let token = extract_bearer(&parts.headers);
        let db = state.inner.db.clone();
        async move {
            let token = token.ok_or(AppError::Unauthorized)?;
            let token_hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();

            let row = sqlx::query_as::<_, TokenRow>(
                "SELECT t.user_id, t.session_id \
                 FROM auth_tokens t \
                 JOIN users u ON u.id = t.user_id \
                 WHERE t.token_hash = $1 AND u.blocked = false",
            )
            .bind(token_hash.as_slice())
            .fetch_optional(&db)
            .await
            .map_err(|_| AppError::Unauthorized)?;

            match row {
                Some(r) => {
                    // Update last_seen without blocking the response.
                    let db2 = db.clone();
                    let sid = r.session_id;
                    tokio::spawn(async move {
                        let _ = sqlx::query(
                            "UPDATE auth_tokens SET last_seen = now() WHERE session_id = $1",
                        )
                        .bind(sid)
                        .execute(&db2)
                        .await;
                    });
                    Ok(AuthUser {
                        user_id: r.user_id.to_string(),
                        session_id: r.session_id.to_string(),
                    })
                }
                None => Err(AppError::Unauthorized),
            }
        }
    }
}

pub fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("authorization")?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    Some(token.to_owned())
}

// ── AdminAuth extractor ───────────────────────────────────────────────────────

/// Validates the `Authorization: Bearer <token>` header against ADMIN_TOKEN env var.
/// Returns 401 if the token doesn't match or ADMIN_TOKEN is not set.
pub struct AdminAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let token = extract_bearer(&parts.headers);
        let hash = state.inner.admin_token_hash.clone();
        async move {
            if hash.is_empty() {
                return Err(AppError::Unauthorized);
            }
            let token = token.ok_or(AppError::Unauthorized)?;
            let provided_hash: Vec<u8> = Sha256::digest(token.as_bytes()).to_vec();
            if provided_hash == hash {
                Ok(AdminAuth)
            } else {
                Err(AppError::Unauthorized)
            }
        }
    }
}

// ── Rate limit middleware ─────────────────────────────────────────────────────

pub async fn rate_limit(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match state.inner.rate_limiter.check_key(&peer.ip()) {
        Ok(_) => next.run(request).await,
        Err(_) => AppError::RateLimited.into_response(),
    }
}
