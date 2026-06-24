use std::{net::IpAddr, num::NonZeroU32, sync::Arc, time::Instant};

use axum::extract::ws::Message;
use dashmap::DashMap;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::nats::JetStream;

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

        let quota = Quota::per_second(NonZeroU32::new(10).expect("10 > 0"));
        Self {
            inner: Arc::new(Inner {
                db,
                nats_client,
                js,
                sessions: DashMap::new(),
                rate_limiter: RateLimiter::keyed(quota),
                admin_token_hash,
                registration_open,
                start_time: Instant::now(),
            }),
        }
    }
}
