use reqwest::Client;
use serde::Deserialize;

use crate::state::{AppState, CachedToken, FcmServiceAccount};

// ── Public entry point ────────────────────────────────────────────────────────

pub async fn send_push_to_user(state: &AppState, user_id: &str) {
    let Some(sa) = &state.inner.fcm_service_account else {
        tracing::debug!("FCM_SERVICE_ACCOUNT_JSON not set — push disabled");
        return;
    };

    let Ok(uid) = uuid::Uuid::parse_str(user_id) else { return };

    let rows = match sqlx::query!(
        "SELECT platform, token FROM push_tokens WHERE user_id = $1",
        uid,
    )
    .fetch_all(&state.inner.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("push_tokens query failed: {e}");
            return;
        }
    };

    // Obtain (or reuse cached) OAuth2 access token once for this batch.
    let access_token = match get_access_token(
        &state.inner.http_client,
        sa,
        &state.inner.fcm_token_cache,
    )
    .await
    {
        Some(t) => t,
        None => return,
    };

    for row in rows {
        match row.platform.as_str() {
            "fcm" => {
                send_fcm_v1(
                    &state.inner.http_client,
                    &row.token,
                    &access_token,
                    &sa.project_id,
                )
                .await;
            }
            "apns" => {
                tracing::debug!("APNs push not yet implemented — skipping");
            }
            other => tracing::warn!(platform = other, "unknown push platform"),
        }
    }
}

// ── OAuth2 token (cached, auto-refreshed) ─────────────────────────────────────

async fn get_access_token(
    client: &Client,
    sa: &FcmServiceAccount,
    cache: &tokio::sync::Mutex<Option<CachedToken>>,
) -> Option<String> {
    let now = unix_now();

    {
        let guard = cache.lock().await;
        if let Some(ref cached) = *guard {
            // Reuse if at least 60 s of lifetime remains.
            if cached.expires_at > now + 60 {
                return Some(cached.token.clone());
            }
        }
    }

    // Need a fresh token — mint a JWT and exchange it.
    let jwt = match mint_jwt(sa, now) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("FCM JWT signing failed: {e}");
            return None;
        }
    };

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        expires_in:   i64,
    }

    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion",  &jwt),
        ])
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            match r.json::<TokenResponse>().await {
                Ok(tok) => {
                    let expires_at = now + tok.expires_in;
                    let token = tok.access_token.clone();
                    let mut guard = cache.lock().await;
                    *guard = Some(CachedToken { token: tok.access_token, expires_at });
                    tracing::debug!("FCM OAuth2 token refreshed, valid for {}s", tok.expires_in);
                    Some(token)
                }
                Err(e) => {
                    tracing::error!("FCM token response parse failed: {e}");
                    None
                }
            }
        }
        Ok(r) => {
            tracing::error!(status = %r.status(), "FCM token exchange rejected");
            None
        }
        Err(e) => {
            tracing::error!("FCM token exchange request failed: {e}");
            None
        }
    }
}

// ── JWT creation (RS256) ──────────────────────────────────────────────────────

fn mint_jwt(sa: &FcmServiceAccount, now: i64) -> Result<String, jsonwebtoken::errors::Error> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use serde::Serialize;

    #[derive(Serialize)]
    struct Claims {
        iss:   String,
        sub:   String,
        aud:   String,
        iat:   i64,
        exp:   i64,
        scope: String,
    }

    let claims = Claims {
        iss:   sa.client_email.clone(),
        sub:   sa.client_email.clone(),
        aud:   "https://oauth2.googleapis.com/token".into(),
        iat:   now,
        exp:   now + 3600,
        scope: "https://www.googleapis.com/auth/firebase.messaging".into(),
    };

    let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())?;
    encode(&Header::new(Algorithm::RS256), &claims, &key)
}

// ── FCM HTTP v1 send ──────────────────────────────────────────────────────────

async fn send_fcm_v1(
    client: &Client,
    device_token: &str,
    access_token: &str,
    project_id: &str,
) {
    let url = format!(
        "https://fcm.googleapis.com/v1/projects/{project_id}/messages:send"
    );

    let body = serde_json::json!({
        "message": {
            "token": device_token,
            "notification": {
                "title": "New message",
                "body":  "You have a new message"
            },
            "android": {
                "priority": "HIGH"
            },
            "data": {
                "type": "new_message"
            }
        }
    });

    match client
        .post(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(device_token = &device_token[..8], "FCM v1 push sent");
        }
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(status = %status, body = %text, "FCM v1 push rejected");
        }
        Err(e) => tracing::warn!("FCM v1 push request failed: {e}"),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
