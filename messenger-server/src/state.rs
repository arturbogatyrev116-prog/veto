use std::{net::IpAddr, num::NonZeroU32, sync::Arc, time::Instant};

use axum::extract::ws::Message;
use dashmap::DashMap;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sqlx::PgPool;
use tokio::sync::{mpsc, Mutex as TokioMutex};

use crate::nats::JetStream;

/// Parsed Firebase service account (loaded from FCM_SERVICE_ACCOUNT_JSON env var).
pub struct FcmServiceAccount {
    pub project_id:   String,
    pub client_email: String,
    pub private_key:  String,
}

/// Cached OAuth2 access token for FCM v1 API.
pub struct CachedToken {
    pub token:      String,
    pub expires_at: i64,  // Unix seconds
}

pub type UserId = String;
pub type Tx = mpsc::UnboundedSender<Message>;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    /// PostgreSQL connection pool — auth tokens + prekey bundles + users.
    pub db: PgPool,

    /// Raw NATS client — used for flush() on graceful shutdown.
    pub nats_client: async_nats::Client,

    /// NATS JetStream context — durable offline message queue.
    pub js: JetStream,

    /// user_id → live WebSocket sender.
    pub sessions: DashMap<UserId, Tx>,

    /// Per-IP rate limiter: 60 requests/minute.
    pub rate_limiter: DefaultKeyedRateLimiter<IpAddr>,

    /// SHA-256 hash of the ADMIN_TOKEN env var. Empty = admin API disabled.
    pub admin_token_hash: Vec<u8>,

    /// When false, POST /api/v1/register returns 403. Set REGISTRATION_OPEN=1 to allow.
    pub registration_open: bool,

    /// Server start timestamp — used by the health endpoint to report uptime.
    pub start_time: Instant,

    /// HTTP client for internal service calls (LibreTranslate, etc.)
    pub http_client: reqwest::Client,

    /// URL of the LibreTranslate instance. None = translation disabled.
    pub libretranslate_url: Option<String>,

    /// Per-user translation rate limiter: 30 requests/minute per user_id.
    pub translate_limiter: DefaultKeyedRateLimiter<String>,

    /// Firebase service account for FCM HTTP v1 API.
    /// Loaded from FCM_SERVICE_ACCOUNT_JSON env var. None = push disabled.
    pub fcm_service_account: Option<FcmServiceAccount>,

    /// Cached OAuth2 access token — refreshed automatically when expired.
    pub fcm_token_cache: TokioMutex<Option<CachedToken>>,
}

impl AppState {
    pub fn new(db: PgPool, nats_client: async_nats::Client, js: JetStream) -> Self {
        use sha2::{Digest, Sha256};
        let admin_token_hash = std::env::var("ADMIN_TOKEN")
            .map(|t| Sha256::digest(t.as_bytes()).to_vec())
            .unwrap_or_default();

        let registration_open = std::env::var("REGISTRATION_OPEN")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        let libretranslate_url = std::env::var("LIBRETRANSLATE_URL").ok();

        let fcm_service_account = std::env::var("FCM_SERVICE_ACCOUNT_JSON").ok()
            .and_then(|raw| {
                #[derive(serde::Deserialize)]
                struct Sa { project_id: String, client_email: String, private_key: String }
                match serde_json::from_str::<Sa>(&raw) {
                    Ok(sa) => Some(FcmServiceAccount {
                        project_id:   sa.project_id,
                        client_email: sa.client_email,
                        private_key:  sa.private_key,
                    }),
                    Err(e) => {
                        tracing::error!("FCM_SERVICE_ACCOUNT_JSON parse failed: {e}");
                        None
                    }
                }
            });
        if fcm_service_account.is_some() {
            tracing::info!("FCM v1 push enabled");
        }

        let global_quota = Quota::per_second(NonZeroU32::new(10).expect("10 > 0"));
        let translate_quota = Quota::per_minute(NonZeroU32::new(30).expect("30 > 0"));
        Self {
            inner: Arc::new(Inner {
                db,
                nats_client,
                js,
                sessions: DashMap::new(),
                rate_limiter: RateLimiter::keyed(global_quota),
                admin_token_hash,
                registration_open,
                start_time: Instant::now(),
                http_client: reqwest::Client::new(),
                libretranslate_url,
                translate_limiter: RateLimiter::keyed(translate_quota),
                fcm_service_account,
                fcm_token_cache: TokioMutex::new(None),
            }),
        }
    }
}
