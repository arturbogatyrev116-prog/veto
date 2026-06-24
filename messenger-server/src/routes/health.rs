use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use std::time::Instant;

use crate::state::AppState;

/// GET /health
///
/// Checks PostgreSQL and NATS connectivity, returns uptime, WS connection count,
/// and DB query latency. Returns 200 when healthy, 503 when DB or NATS is down.
pub async fn handler(State(state): State<AppState>) -> impl IntoResponse {
    let db_start = Instant::now();
    let db_ok = sqlx::query("SELECT 1")
        .execute(&state.inner.db)
        .await
        .is_ok();
    let db_latency_ms = db_start.elapsed().as_millis() as u64;

    let nats_ok = state.inner.js.get_stream("MESSAGES").await.is_ok();

    let http_status = if db_ok && nats_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let uptime_secs = state.inner.start_time.elapsed().as_secs();
    let active_ws = state.inner.sessions.len();
    let pool_size = state.inner.db.size();

    (
        http_status,
        Json(json!({
            "status":                  if db_ok && nats_ok { "ok" } else { "degraded" },
            "uptime_secs":             uptime_secs,
            "db":                      if db_ok   { "ok" } else { "error" },
            "db_latency_ms":           db_latency_ms,
            "db_pool_size":            pool_size,
            "nats":                    if nats_ok { "ok" } else { "error" },
            "active_ws_connections":   active_ws,
            "version":                 env!("CARGO_PKG_VERSION"),
        })),
    )
}
