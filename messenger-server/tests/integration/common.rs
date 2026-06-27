use std::net::SocketAddr;

use futures_util::SinkExt;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use messenger_server::{nats, routes, state::AppState};

pub async fn spawn_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://messenger:secret@localhost/messenger".into());

    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect to test DB");

    let nats_url = std::env::var("NATS_URL")
        .unwrap_or_else(|_| "nats://localhost:4222".into());
    let (nats_client, js) = nats::connect_and_setup(&nats_url).await;

    let state = AppState::new(db, nats_client, js);
    let app = routes::router(state.clone())
        .with_state(state)
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (addr, handle)
}

/// reqwest client with proxy disabled — Windows system proxy intercepts 127.0.0.1 otherwise.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
}

pub async fn register(addr: SocketAddr, username: &str) -> (String, String) {
    // Unique suffix avoids conflicts across test runs (PG persists between runs).
    let unique = format!("{}_{}", username, unique_id());
    let resp = http_client()
        .post(format!("http://{addr}/api/v1/register"))
        .json(&serde_json::json!({ "username": unique }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 201, "register '{unique}' failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    (
        body["user_id"].as_str().unwrap().to_owned(),
        body["token"].as_str().unwrap().to_owned(),
    )
}

/// Connect to the WebSocket and authenticate via first-message auth frame.
/// Returns the authenticated WebSocket stream ready for use.
pub async fn ws_connect(
    addr: SocketAddr,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("ws connect");
    ws.send(WsMessage::Text(
        serde_json::json!({ "type": "auth", "token": token }).to_string().into(),
    ))
    .await
    .expect("ws auth frame");
    ws
}

fn unique_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}
