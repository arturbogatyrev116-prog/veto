use std::net::SocketAddr;

use axum::{
    http::{HeaderName, HeaderValue, Method},
    Router,
    extract::DefaultBodyLimit,
};
use axum_server::tls_rustls::RustlsConfig;
use messenger_server::{routes, state::AppState};
use rcgen::{CertificateParams, DistinguishedName, DnType};
use sqlx::postgres::PgPoolOptions;
use tower_http::{
    cors::CorsLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::{ServeDir, ServeFile},
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const REQUEST_ID_HEADER: &str = "x-request-id";

enum TlsMode {
    Plain,
    SelfSigned,
    Files { cert: String, key: String },
}

#[tokio::main]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls ring provider");

    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "messenger_server=debug,tower_http=debug"
                    .parse()
                    .expect("valid filter")
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://messenger:secret@localhost/messenger".into());

    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&db_url)
        .await
        .expect("failed to connect to PostgreSQL");

    tracing::info!("connected to PostgreSQL");

    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("failed to run database migrations");
    tracing::info!("migrations applied");

    let nats_url = std::env::var("NATS_URL")
        .unwrap_or_else(|_| "nats://localhost:4222".into());
    let (nats_client, js) = messenger_server::nats::connect_and_setup(&nats_url).await;
    tracing::info!("connected to NATS JetStream");

    let state = AppState::new(db, nats_client.clone(), js);
    let request_id_header = HeaderName::from_static(REQUEST_ID_HEADER);

    // CORS — only allow the Tauri app origin (and optional override via CORS_ALLOWED_ORIGIN).
    // The desktop client uses tauri://localhost (macOS/Linux) or https://tauri.localhost (Windows).
    let cors = {
        let mut allowed: Vec<HeaderValue> = vec![
            "tauri://localhost".parse().unwrap(),
            "https://tauri.localhost".parse().unwrap(),
        ];
        if let Ok(extra) = std::env::var("CORS_ALLOWED_ORIGIN") {
            if let Ok(v) = extra.parse::<HeaderValue>() {
                allowed.push(v);
            }
        }
        CorsLayer::new()
            .allow_origin(allowed)
            .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::PUT, Method::DELETE])
            .allow_headers([
                axum::http::header::CONTENT_TYPE,
                axum::http::header::AUTHORIZATION,
            ])
            .allow_credentials(false)
    };

    // Serve admin UI static files. Falls back to index.html for SPA routing.
    let admin_ui_dir = std::env::var("ADMIN_UI_DIR")
        .unwrap_or_else(|_| "admin-ui/dist".into());
    let admin_index = format!("{admin_ui_dir}/index.html");

    let app = Router::new()
        .merge(routes::router(state.clone()))
        .layer(cors)
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .nest_service(
            "/admin",
            ServeDir::new(&admin_ui_dir)
                .not_found_service(ServeFile::new(&admin_index)),
        )
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new()
                        .level(Level::INFO)
                        .include_headers(false),
                )
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .with_state(state);

    let bind_addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".into())
        .parse()
        .expect("BIND_ADDR must be a valid socket address");

    let tls_mode = match (
        std::env::var("TLS_CERT_PATH"),
        std::env::var("TLS_KEY_PATH"),
        std::env::var("TLS_SELF_SIGNED"),
    ) {
        (Ok(cert), Ok(key), _) => TlsMode::Files { cert, key },
        (_, _, Ok(_)) => TlsMode::SelfSigned,
        _ => TlsMode::Plain,
    };

    tracing::info!("listening on {bind_addr}");

    match tls_mode {
        TlsMode::Plain => {
            let listener = tokio::net::TcpListener::bind(bind_addr)
                .await
                .expect("failed to bind TCP listener");

            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("server error");
        }

        TlsMode::SelfSigned => {
            let (cert_pem, key_pem) = generate_self_signed();
            std::fs::write("cert.pem", &cert_pem).expect("write cert.pem");
            std::fs::write("key.pem", &key_pem).expect("write key.pem");
            tracing::info!("self-signed cert written to cert.pem / key.pem");
            tracing::info!(
                "clients on other machines need MESSENGER_INSECURE_TLS=1 \
                 or install cert.pem in their trust store"
            );

            serve_tls(app, bind_addr, cert_pem.into_bytes(), key_pem.into_bytes()).await;
        }

        TlsMode::Files { cert, key } => {
            tracing::info!("loading TLS cert from {cert}, key from {key}");
            let cert_pem = std::fs::read(&cert).expect("read cert file");
            let key_pem = std::fs::read(&key).expect("read key file");
            serve_tls(app, bind_addr, cert_pem, key_pem).await;
        }
    }

    // Flush in-flight NATS publishes before the runtime exits.
    nats_client.flush().await.ok();
    tracing::info!("NATS flushed, exiting");
}

async fn serve_tls(
    app: axum::Router,
    bind_addr: SocketAddr,
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
) {
    let config = RustlsConfig::from_pem(cert_pem, key_pem)
        .await
        .expect("failed to build TLS config");

    // axum_server does not have .with_graceful_shutdown(); use Handle instead.
    let handle = axum_server::Handle::new();
    let h2 = handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        h2.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
    });

    axum_server::bind_rustls(bind_addr, config)
        .handle(handle)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .expect("TLS server error");
}

fn generate_self_signed() -> (String, String) {
    let mut params = CertificateParams::new(vec!["localhost".to_string()]);
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::CommonName, "Messenger Dev");
    let cert = rcgen::Certificate::from_params(params).expect("cert generation failed");
    let cert_pem = cert.serialize_pem().expect("serialize cert");
    let key_pem = cert.serialize_private_key_pem();
    (cert_pem, key_pem)
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl-c");
    tracing::info!("shutdown signal received, draining connections…");
}
