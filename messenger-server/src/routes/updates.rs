use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Deserialize)]
struct LatestJson {
    version: String,
    notes: Option<String>,
    pub_date: Option<String>,
    platforms: HashMap<String, PlatformEntry>,
}

#[derive(Deserialize)]
struct PlatformEntry {
    url: String,
    signature: String,
}

#[derive(Serialize)]
struct UpdateResponse {
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub_date: Option<String>,
    url: String,
    signature: String,
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let v = v.trim_start_matches('v');
        let mut p = v.splitn(3, '.');
        Some((
            p.next()?.parse().ok()?,
            p.next()?.parse().ok()?,
            p.next()?.split('-').next()?.parse().ok()?,
        ))
    };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// `GET /api/v1/updates/:target/:current_version`
///
/// Returns 204 when no update is available, 200 + JSON when an update exists.
/// Reads `$UPDATES_DIR/latest.json` (default: `updates/latest.json`).
pub async fn check(Path((target, current_version)): Path<(String, String)>) -> Response {
    let dir = std::env::var("UPDATES_DIR").unwrap_or_else(|_| "updates".into());
    let path = std::path::Path::new(&dir).join("latest.json");

    let content = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => return StatusCode::NO_CONTENT.into_response(),
    };

    let latest: LatestJson = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if !is_newer(&latest.version, &current_version) {
        return StatusCode::NO_CONTENT.into_response();
    }

    let platform = match latest.platforms.get(&target) {
        Some(p) => p,
        None => return StatusCode::NO_CONTENT.into_response(),
    };

    Json(UpdateResponse {
        version: latest.version,
        notes: latest.notes,
        pub_date: latest.pub_date,
        url: platform.url.clone(),
        signature: platform.signature.clone(),
    })
    .into_response()
}
