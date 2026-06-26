use axum::{extract::State, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{auth::AuthUser, error::AppError, state::AppState};

const CACHE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000; // 7 days

#[derive(Deserialize)]
pub struct TranslateRequest {
    pub text: String,
    pub target_lang: String,
}

#[derive(Serialize)]
pub struct TranslateResponse {
    pub translated_text: String,
    pub cached: bool,
}

fn cache_key(target_lang: &str, text: &str) -> String {
    let mut h = Sha256::new();
    h.update(target_lang.as_bytes());
    h.update(b":");
    h.update(text.as_bytes());
    hex::encode(h.finalize())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// POST /api/v1/translate  (JWT required)
///
/// Checks PostgreSQL cache first (7-day TTL). On miss, proxies to the on-premise
/// LibreTranslate instance (LIBRETRANSLATE_URL env var) and stores the result.
/// Rate-limited to 30 translations per minute per user.
pub async fn handler(
    auth: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<TranslateRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.text.is_empty() {
        return Ok(Json(TranslateResponse { translated_text: String::new(), cached: false }));
    }

    // Per-user rate limit: 30 requests/minute
    if state.inner.translate_limiter.check_key(&auth.user_id).is_err() {
        return Err(AppError::RateLimited);
    }

    let key = cache_key(&body.target_lang, &body.text);
    let cutoff = now_ms() - CACHE_TTL_MS;

    // Cache lookup
    let cached: Option<String> = sqlx::query_scalar(
        "SELECT translated FROM translation_cache WHERE cache_key = $1 AND cached_at > $2"
    )
    .bind(&key)
    .bind(cutoff)
    .fetch_optional(&state.inner.db)
    .await?;

    if let Some(translated) = cached {
        return Ok(Json(TranslateResponse { translated_text: translated, cached: true }));
    }

    // Translation service required for a cache miss
    let lt_url = state.inner.libretranslate_url.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable(
            "translation service not configured (set LIBRETRANSLATE_URL)".into(),
        )
    })?;

    let endpoint = format!("{}/translate", lt_url.trim_end_matches('/'));

    let resp: serde_json::Value = state.inner.http_client
        .post(&endpoint)
        .json(&serde_json::json!({
            "q":      body.text,
            "source": "auto",
            "target": body.target_lang,
            "format": "text"
        }))
        .send()
        .await
        .map_err(|e| AppError::BadGateway(format!("libretranslate: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::BadGateway(format!("libretranslate parse: {e}")))?;

    let translated = resp["translatedText"]
        .as_str()
        .ok_or_else(|| {
            let err = resp["error"].as_str().unwrap_or("unexpected response");
            AppError::BadGateway(format!("libretranslate error: {err}"))
        })?
        .to_owned();

    // Store in cache (best-effort — ignore errors)
    let _ = sqlx::query(
        "INSERT INTO translation_cache (cache_key, target_lang, source_text, translated, cached_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (cache_key) DO UPDATE SET translated = $4, cached_at = $5"
    )
    .bind(&key)
    .bind(&body.target_lang)
    .bind(&body.text)
    .bind(&translated)
    .bind(now_ms())
    .execute(&state.inner.db)
    .await;

    Ok(Json(TranslateResponse { translated_text: translated, cached: false }))
}
