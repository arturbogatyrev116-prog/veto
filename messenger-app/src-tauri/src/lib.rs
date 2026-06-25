use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::AtomicU32, Mutex},
};

// ── Group types ──────────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct MemberInfo {
    pub user_id: String,
    pub username: String,
    #[serde(default = "default_member_role")]
    pub role: String,
}

fn default_member_role() -> String { "member".to_string() }

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct GroupInfo {
    pub group_id: String,
    pub name: String,
    pub members: Vec<MemberInfo>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelInfo {
    pub channel_id:  String,
    pub group_id:    String,
    pub name:        String,
    pub description: Option<String>,
    pub subscribed:  bool,
}

use messenger_crypto::{ratchet::RatchetState, x3dh::X3dhHeader};
use tokio::sync::mpsc;

pub mod client;
pub mod commands;
pub mod db;
pub mod store;

// ── Associated data ──────────────────────────────────────────────────────────

pub const AD: &[u8] = b"messenger-v1";

// ── State ────────────────────────────────────────────────────────────────────

pub struct IdentityState {
    pub user_id: String,
    pub username: String,
    pub token: String,
    /// Raw bytes — reconstructed into crypto types on demand so we don't need
    /// to worry about Send/Sync bounds on ZeroizeOnDrop key types.
    pub signing_key_bytes: [u8; 32],
    pub spk_secret_bytes: [u8; 32],
    pub spk_id: u32,
    /// Unix ms of last SPK upload — used to decide when to rotate.
    pub spk_rotation_ts: i64,
    /// One-time prekey private key pool: (opk_id, secret_bytes).
    pub opk_secret_bytes: Vec<(u32, [u8; 32])>,
    /// Next OPK id to use when generating a replenishment batch.
    pub opk_next_id: u32,
    /// ML-KEM-768 decapsulation key bytes (2400 bytes). Empty if no PQ SPK registered yet.
    pub pq_spk_secret_bytes: Vec<u8>,
}

pub enum PeerSession {
    /// Alice initiated: X3DH done, first message not yet sent.
    /// `pq_ct` carries the ML-KEM-768 ciphertext to include in InitEnvelope (None if no PQ).
    AlicePending { x3dh_header: X3dhHeader, pq_ct: Option<Vec<u8>>, ratchet: RatchetState },
    /// Session established (either role).
    Established { ratchet: RatchetState },
}

/// Shared application state managed by Tauri.
/// All fields use std::sync::Mutex — crypto operations are sync and fast (µs),
/// so we never hold a guard across an .await point.
pub struct AppState {
    pub identity: Mutex<Option<IdentityState>>,
    pub sessions: Mutex<HashMap<String, PeerSession>>,
    pub ws_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// 32-byte session encryption key derived from the user's password via Argon2id.
    pub session_key: Mutex<Option<[u8; 32]>>,
    /// Peer X25519 identity keys — stored on first contact for safety number verification.
    /// Alice populates on prepare_session; Bob on receiving InitEnvelope.
    pub peer_identity_keys: Mutex<HashMap<String, [u8; 32]>>,
    /// Monotonic per-session message counter used as msg_id for delivery ACKs.
    pub msg_counter: AtomicU32,
    /// SQLite connection for local message history. Opened on `unlock`, closed on `clear_identity`.
    pub db: Mutex<Option<rusqlite::Connection>>,
    /// Encrypted frames awaiting delivery ACK. Flushed to new WS on reconnect; ACK removes entry.
    pub outgoing_queue: Mutex<VecDeque<(u32, Vec<u8>)>>,
    /// Known group chats — populated by `create_group` and `load_groups`.
    pub groups: Mutex<HashMap<String, GroupInfo>>,
    /// Channels per group — populated by `load_channels`.
    /// Key = channel_id, Value = ChannelInfo.
    pub channels: Mutex<HashMap<String, ChannelInfo>>,
    /// Peers to whom a read receipt must be sent on next successful connect.
    pub pending_receipts: Mutex<HashSet<String>>,
    /// Display names for peers — populated by prepare_session; used in notifications.
    pub peer_names: Mutex<HashMap<String, String>>,
    /// When true: accept self-signed / invalid TLS certs (set via MESSENGER_INSECURE_TLS).
    /// Intended for LAN testing with self-signed certs. Never set in production.
    pub accept_invalid_certs: bool,
    pub http: reqwest::Client,
    pub server_url: String,
    /// Context stored when showing a native popup context menu; read by on_menu_event handler.
    pub ctx_menu_context: Mutex<Option<commands::CtxMenuContext>>,
}

impl AppState {
    fn new(server_url: String) -> Self {
        let accept_invalid_certs = std::env::var("MESSENGER_INSECURE_TLS").is_ok();
        let http = reqwest::Client::builder()
            .no_proxy()
            .danger_accept_invalid_certs(accept_invalid_certs)
            .build()
            .expect("reqwest client");
        Self {
            identity: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            ws_tx: Mutex::new(None),
            session_key: Mutex::new(None),
            peer_identity_keys: Mutex::new(HashMap::new()),
            msg_counter: AtomicU32::new(1),
            db: Mutex::new(None),
            outgoing_queue: Mutex::new(VecDeque::new()),
            groups: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
            pending_receipts: Mutex::new(HashSet::new()),
            peer_names: Mutex::new(HashMap::new()),
            accept_invalid_certs,
            http,
            server_url,
            ctx_menu_context: Mutex::new(None),
        }
    }
}

// ── Auto-update helpers ──────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
struct UpdateAvailablePayload {
    version: String,
    notes: Option<String>,
}

async fn check_for_update(app: &tauri::AppHandle) {
    use tauri::Emitter;
    use tauri_plugin_updater::UpdaterExt;
    let Ok(updater) = app.updater() else { return };
    let Ok(Some(update)) = updater.check().await else { return };
    let _ = app.emit(
        "update_available",
        UpdateAvailablePayload {
            version: update.version.clone(),
            notes: update.body.clone(),
        },
    );
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    // On Windows the MSI/NSIS installer relaunches the app automatically.
    // On other platforms the user relaunches manually after exit.
    app.exit(0);
    Ok(())
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
        Manager, WindowEvent,
    };
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

    // Priority: env var → AppData config → server_url.txt next to exe → cwd → default
    let server_url = std::env::var("MESSENGER_SERVER_URL").unwrap_or_else(|_| {
        let from_config = commands::server_url_config_path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok());
        let from_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("server_url.txt")))
            .and_then(|p| std::fs::read_to_string(p).ok());
        let from_cwd = std::env::current_dir()
            .ok()
            .and_then(|d| std::fs::read_to_string(d.join("server_url.txt")).ok());
        from_config
            .or(from_exe)
            .or(from_cwd)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "http://localhost:3000".into())
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::new(server_url))
        .setup(|app| {
            // Background update-check loop: runs every hour, emits "update_available" if found.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    check_for_update(&handle).await;
                    tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
                }
            });
            let show_item = MenuItem::with_id(app, "show", "Open Messenger", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Veto")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left-click on tray icon → show and focus window.
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Global hotkey: Ctrl+Shift+V → show/focus window from anywhere
            let shortcut = Shortcut::new(
                Some(Modifiers::CONTROL | Modifiers::SHIFT),
                Code::KeyM,
            );
            let handle = app.handle().clone();
            if let Err(e) = app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, _event| {
                if let Some(w) = handle.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }) {
                tracing::warn!("Could not register global shortcut Ctrl+Shift+M: {e}");
            }

            Ok(())
        })
        .on_menu_event(|app, event| {
            use tauri::Emitter;
            let id = event.id.0.as_str();
            if !id.starts_with("ctx_") { return; }
            let state = app.state::<AppState>();
            let ctx = state.ctx_menu_context.lock().unwrap().clone();
            if let Some(ctx) = ctx {
                let _ = app.emit("ctx_menu_action", serde_json::json!({ "action": id, "ctx": ctx }));
            }
        })
        .on_window_event(|window, event| {
            // Intercept window close → hide to system tray instead of quitting.
            // The app can only be fully exited via the tray menu "Quit".
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_identity,
            commands::register,
            commands::connect,
            commands::unlock,
            commands::clear_identity,
            commands::prepare_session,
            commands::send_message,
            commands::send_file,
            commands::download_file,
            commands::get_messages,
            commands::get_safety_number,
            commands::send_typing,
            commands::send_read_receipt,
            commands::export_identity,
            commands::import_identity,
            commands::create_group,
            commands::load_groups,
            commands::send_group_message,
            commands::distribute_sender_key,
            commands::group_has_sender_key,
            commands::send_group_file,
            commands::get_group_messages,
            commands::leave_group,
            commands::set_member_role,
            commands::kick_member,
            commands::transfer_ownership,
            commands::update_group_name,
            commands::load_channels,
            commands::create_channel,
            commands::delete_channel,
            commands::subscribe_channel,
            commands::unsubscribe_channel,
            commands::send_channel_message,
            commands::translate_message,
            commands::send_rsvp,
            commands::search_messages,
            commands::mark_as_read,
            commands::get_last_read_ts,
            commands::get_unread_counts,
            commands::send_reaction,
            commands::get_reactions,
            commands::list_sessions,
            commands::revoke_session,
            commands::get_device_id,
            commands::save_draft,
            commands::get_draft,
            commands::set_mute,
            commands::get_mute,
            commands::set_ttl,
            commands::get_ttl,
            commands::edit_message,
            commands::delete_message,
            commands::get_edit_history,
            commands::send_group_read_receipt,
            commands::get_group_read_marks,
            commands::export_chat,
            commands::get_server_url,
            commands::set_server_url,
            commands::update_profile,
            commands::pin_message,
            commands::unpin_message,
            commands::get_pinned_messages,
            commands::save_note,
            commands::get_chat_stats,
            commands::set_screen_capture_protection,
            commands::show_message_context_menu,
            commands::show_notification,
            commands::has_biometric_unlock,
            commands::save_biometric_unlock,
            commands::delete_biometric_unlock,
            commands::try_biometric_unlock,
            commands::create_poll,
            commands::get_poll,
            commands::vote_poll,
            commands::close_poll,
            commands::schedule_message,
            commands::list_scheduled,
            commands::cancel_scheduled,
            commands::fetch_link_preview,
            commands::set_retention,
            commands::get_retention,
            install_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
