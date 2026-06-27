use rusqlite::{params, Connection, Error as SqlError};
use std::path::Path;

// ── Schema ────────────────────────────────────────────────────────────────────

// rusqlite 0.32 changed execute_batch to use prepare/step instead of sqlite3_exec.
// Some DDL statements (FTS5 virtual tables, certain indexed expressions) return
// SQLITE_ROW during initialization, which triggers ExecuteReturnedResults.
// This helper treats that as success — the statement ran, it just returned a row.
fn exec(conn: &Connection, sql: &str) -> Result<(), String> {
    conn.execute_batch(sql).or_else(|e| {
        if matches!(e, SqlError::ExecuteReturnedResults) {
            // DDL returned a row (FTS5 or indexed expr init) — the statement ran OK.
            Ok(())
        } else {
            let preview: String = sql.chars().take(80).collect();
            Err(format!("db::open SQL error [{preview}]: {e}"))
        }
    })
}

/// Encrypt a plaintext `messages.db` in place using SQLCipher.
/// Creates `messages.db.new` (encrypted), then renames `messages.db` → `messages.db.bak`
/// and `messages.db.new` → `messages.db`. Idempotent: marker `.db_encrypted` guards re-runs.
#[cfg(feature = "sqlcipher")]
fn migrate_to_sqlcipher(data_dir: &Path, key: &[u8; 32]) -> Result<(), String> {
    let db_path  = data_dir.join("messages.db");
    let new_path = data_dir.join("messages.db.new");
    let bak_path = data_dir.join("messages.db.bak");

    // Open old plaintext DB (no key set = SQLCipher plaintext-compat mode).
    let old_conn = Connection::open(&db_path)
        .map_err(|e| format!("sqlcipher migration: open old db: {e}"))?;

    // SQLite paths must use forward slashes (even on Windows).
    let new_path_str = new_path
        .to_str()
        .ok_or_else(|| "sqlcipher migration: non-UTF8 path".to_string())?
        .replace('\\', "/")
        .replace('\'', "");     // guard against unlikely ' in path
    let hex_key = hex::encode(key);

    old_conn
        .execute_batch(&format!(
            "ATTACH DATABASE '{new_path_str}' AS encrypted KEY \"x'{hex_key}'\";"
        ))
        .map_err(|e| format!("sqlcipher migration: ATTACH: {e}"))?;

    old_conn
        .execute_batch("SELECT sqlcipher_export('encrypted');")
        .map_err(|e| format!("sqlcipher migration: export: {e}"))?;

    old_conn
        .execute_batch("DETACH DATABASE encrypted;")
        .map_err(|e| format!("sqlcipher migration: DETACH: {e}"))?;

    drop(old_conn);

    std::fs::rename(&db_path, &bak_path)
        .map_err(|e| format!("sqlcipher migration: rename old: {e}"))?;
    std::fs::rename(&new_path, &db_path)
        .map_err(|e| format!("sqlcipher migration: rename new: {e}"))?;

    Ok(())
}

pub fn open(data_dir: &Path, key: &[u8; 32]) -> Result<Connection, String> {
    let db_path = data_dir.join("messages.db");

    // Without the `sqlcipher` feature, the key is only used in the FTS build path.
    // Suppress the compiler warning for the non-encrypted build.
    #[cfg(not(feature = "sqlcipher"))]
    let _ = key;

    #[cfg(feature = "sqlcipher")]
    {
        // One-time migration: encrypt an existing plaintext DB.
        let marker = data_dir.join(".db_encrypted");
        if db_path.exists() && !marker.exists() {
            migrate_to_sqlcipher(data_dir, key)?;
            std::fs::File::create(&marker)
                .map_err(|e| format!("db::open: create encryption marker: {e}"))?;
        }
    }

    let conn = Connection::open(&db_path)
        .map_err(|e| format!("db::open connect: {e}"))?;

    // PRAGMA key must be the first SQL issued on the connection (SQLCipher only).
    #[cfg(feature = "sqlcipher")]
    {
        let hex_key = hex::encode(key);
        conn.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))
            .map_err(|e| format!("db::open: SQLCipher key: {e}"))?;
    }

    exec(&conn, "PRAGMA journal_mode=WAL;")?;

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS messages (
             id        INTEGER PRIMARY KEY AUTOINCREMENT,
             peer_id   TEXT    NOT NULL,
             direction TEXT    NOT NULL,
             ts        INTEGER NOT NULL,
             nonce     BLOB    NOT NULL,
             ct        BLOB    NOT NULL,
             status    TEXT    NOT NULL DEFAULT 'sent',
             smid      INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_msgs_peer ON messages(peer_id, id ASC);
         CREATE INDEX IF NOT EXISTS idx_msgs_smid ON messages(smid) WHERE smid IS NOT NULL;",
    )?;

    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN msg_hash BLOB;");
    exec(&conn,
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_msgs_hash \
         ON messages(msg_hash) WHERE msg_hash IS NOT NULL;",
    )?;

    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN reply_to_ts   INTEGER;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN reply_to_from TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN reply_to_text TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN file_id   TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN file_name TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN file_mime TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN file_size INTEGER;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN group_id  TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN sender_id TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN plain      TEXT;");
    let _ = conn.execute_batch("ALTER TABLE messages ADD COLUMN thumb_data BLOB;");

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS conv_meta (\
            peer_id              TEXT    PRIMARY KEY, \
            last_read_ts         INTEGER NOT NULL DEFAULT 0, \
            draft                TEXT    NOT NULL DEFAULT '', \
            notifications_enabled INTEGER NOT NULL DEFAULT 1, \
            mute_until           INTEGER NOT NULL DEFAULT 0, \
            ttl_seconds          INTEGER NOT NULL DEFAULT 0\
        );",
    )?;

    let _ = conn.execute_batch("ALTER TABLE conv_meta ADD COLUMN draft                  TEXT    NOT NULL DEFAULT '';");
    let _ = conn.execute_batch("ALTER TABLE conv_meta ADD COLUMN notifications_enabled  INTEGER NOT NULL DEFAULT 1;");
    let _ = conn.execute_batch("ALTER TABLE conv_meta ADD COLUMN mute_until             INTEGER NOT NULL DEFAULT 0;");
    let _ = conn.execute_batch("ALTER TABLE conv_meta ADD COLUMN ttl_seconds            INTEGER NOT NULL DEFAULT 0;");
    let _ = conn.execute_batch("ALTER TABLE messages  ADD COLUMN edited_at   INTEGER;");
    let _ = conn.execute_batch("ALTER TABLE messages  ADD COLUMN edit_count  INTEGER NOT NULL DEFAULT 0;");
    let _ = conn.execute_batch("ALTER TABLE messages  ADD COLUMN mentions    TEXT;"); // JSON array of user_id strings
    let _ = conn.execute_batch("ALTER TABLE messages  ADD COLUMN thread_parent_ts   INTEGER;");
    let _ = conn.execute_batch("ALTER TABLE messages  ADD COLUMN thread_parent_from TEXT;");

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS message_edits (\
            msg_id    INTEGER NOT NULL, \
            old_plain TEXT    NOT NULL, \
            edited_at INTEGER NOT NULL, \
            PRIMARY KEY (msg_id, edited_at)\
        );",
    )?;

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS reactions (\
            peer_id    TEXT    NOT NULL, \
            msg_ts     INTEGER NOT NULL, \
            msg_from   TEXT    NOT NULL, \
            reactor_id TEXT    NOT NULL, \
            emoji      TEXT    NOT NULL, \
            PRIMARY KEY (peer_id, msg_ts, msg_from, reactor_id)\
        );",
    )?;

    // Security migration: remove plaintext FTS that stored decrypted messages on disk.
    // Plaintext search now lives in an in-memory FTS built at unlock time.
    let _ = conn.execute_batch("
        DROP TRIGGER IF EXISTS messages_ai;
        DROP TRIGGER IF EXISTS messages_au;
        DROP TRIGGER IF EXISTS messages_ad;
        DROP TABLE  IF EXISTS messages_fts;
        UPDATE messages SET plain = NULL;
    ");

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS sender_keys (\
            group_id    TEXT    NOT NULL, \
            user_id     TEXT    NOT NULL, \
            chain_key   BLOB    NOT NULL, \
            counter     INTEGER NOT NULL DEFAULT 0, \
            distributed INTEGER NOT NULL DEFAULT 0, \
            PRIMARY KEY (group_id, user_id)\
        );",
    )?;

    let _ = conn.execute_batch("ALTER TABLE sender_keys ADD COLUMN distributed INTEGER NOT NULL DEFAULT 0;");

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS group_read_marks (\
            group_id TEXT    NOT NULL, \
            user_id  TEXT    NOT NULL, \
            ts       INTEGER NOT NULL DEFAULT 0, \
            PRIMARY KEY (group_id, user_id)\
        );",
    )?;

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS key_log_state (\
            user_id    TEXT    PRIMARY KEY, \
            last_id    INTEGER NOT NULL DEFAULT 0, \
            last_hash  BLOB, \
            updated_at INTEGER NOT NULL DEFAULT 0\
        );",
    )?;

    exec(&conn,
        "CREATE TABLE IF NOT EXISTS pinned_messages (\
            peer_id    TEXT    NOT NULL, \
            msg_ts     INTEGER NOT NULL, \
            msg_from   TEXT    NOT NULL, \
            msg_text   TEXT    NOT NULL DEFAULT '', \
            pinned_at  INTEGER NOT NULL, \
            PRIMARY KEY (peer_id, msg_ts, msg_from)\
        );",
    )?;

    // Channels: sub-rooms within groups.
    exec(&conn,
        "CREATE TABLE IF NOT EXISTS channels (\
            channel_id  TEXT    PRIMARY KEY, \
            group_id    TEXT    NOT NULL, \
            name        TEXT    NOT NULL, \
            description TEXT, \
            subscribed  INTEGER NOT NULL DEFAULT 1 \
        );",
    )?;

    // Scheduled messages queue.
    exec(&conn,
        "CREATE TABLE IF NOT EXISTS scheduled_messages (\
            id               INTEGER PRIMARY KEY AUTOINCREMENT, \
            peer_id          TEXT    NOT NULL, \
            is_group         INTEGER NOT NULL DEFAULT 0, \
            is_channel       INTEGER NOT NULL DEFAULT 0, \
            channel_group_id TEXT, \
            text             TEXT    NOT NULL, \
            reply_to         TEXT, \
            mentions         TEXT, \
            send_at_ms       INTEGER NOT NULL, \
            status           TEXT    NOT NULL DEFAULT 'pending', \
            error            TEXT, \
            created_at       INTEGER NOT NULL \
        );",
    )?;
    exec(&conn,
        "CREATE INDEX IF NOT EXISTS idx_sched_time ON scheduled_messages(send_at_ms, status);",
    )?;

    // C3 Link preview cache
    exec(&conn,
        "CREATE TABLE IF NOT EXISTS link_previews (\
            url         TEXT    PRIMARY KEY, \
            title       TEXT, \
            description TEXT, \
            image_url   TEXT, \
            domain      TEXT    NOT NULL, \
            fetched_at  INTEGER NOT NULL \
        );",
    )?;

    // C15 Performance index for retention queries (group messages share the messages table)
    exec(&conn,
        "CREATE INDEX IF NOT EXISTS idx_messages_peer_ts ON messages(peer_id, ts DESC);",
    )?;

    // C15 Retention column migration (idempotent)
    let _ = conn.execute(
        "ALTER TABLE conv_meta ADD COLUMN retention_count INTEGER NOT NULL DEFAULT 0", [],
    );

    Ok(conn)
}

// ── Writes ────────────────────────────────────────────────────────────────────

pub fn insert_sent(
    conn: &Connection,
    peer_id: &str,
    ts: i64,
    nonce: &[u8; 12],
    ct: &[u8],
    smid: u32,
    reply_to_ts: Option<i64>,
    reply_to_from: Option<&str>,
    reply_to_text: Option<&str>,
    file_id: Option<&str>,
    file_name: Option<&str>,
    file_mime: Option<&str>,
    file_size: Option<i64>,
    thumb_data: Option<&[u8]>,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO messages \
         (peer_id, direction, ts, nonce, ct, status, smid, \
          reply_to_ts, reply_to_from, reply_to_text, \
          file_id, file_name, file_mime, file_size, thumb_data) \
         VALUES (?1, 'sent', ?2, ?3, ?4, 'sent', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![peer_id, ts, nonce.as_slice(), ct, smid,
                reply_to_ts, reply_to_from, reply_to_text,
                file_id, file_name, file_mime, file_size, thumb_data],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Insert a received message, identified by its wire-frame SHA-256 hash.
/// Uses INSERT OR IGNORE so duplicate frames are silently dropped.
/// Returns `true` if the row was inserted, `false` if it was a duplicate.
pub fn insert_received(
    conn: &Connection,
    peer_id: &str,
    ts: i64,
    nonce: &[u8; 12],
    ct: &[u8],
    wire_hash: Option<&[u8; 32]>,
    reply_to_ts: Option<i64>,
    reply_to_from: Option<&str>,
    reply_to_text: Option<&str>,
    file_id: Option<&str>,
    file_name: Option<&str>,
    file_mime: Option<&str>,
    file_size: Option<i64>,
    thumb_data: Option<&[u8]>,
    mentions: Option<&str>,
) -> Result<Option<i64>, String> {
    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO messages \
             (peer_id, direction, ts, nonce, ct, status, msg_hash, \
              reply_to_ts, reply_to_from, reply_to_text, \
              file_id, file_name, file_mime, file_size, thumb_data, mentions) \
             VALUES (?1, 'received', ?2, ?3, ?4, 'delivered', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![peer_id, ts, nonce.as_slice(), ct,
                    wire_hash.map(|h| h.as_slice()),
                    reply_to_ts, reply_to_from, reply_to_text,
                    file_id, file_name, file_mime, file_size, thumb_data, mentions],
        )
        .map_err(|e| e.to_string())?;
    if rows > 0 { Ok(Some(conn.last_insert_rowid())) } else { Ok(None) }
}

/// Check whether a wire frame (identified by its SHA-256 hash) was already stored.
/// Called BEFORE crypto to skip decrypt on exact-duplicate frames.
pub fn is_wire_frame_seen(conn: &Connection, hash: &[u8; 32]) -> bool {
    conn.query_row(
        "SELECT 1 FROM messages WHERE msg_hash = ?1 LIMIT 1",
        params![hash.as_slice()],
        |_| Ok(()),
    )
    .is_ok()
}

/// Mark a sent message as delivered. Returns the `peer_id` so the caller can
/// emit the right event to the frontend. Returns `None` if `smid` not found.
pub fn set_delivered(conn: &Connection, smid: u32) -> Option<String> {
    let peer_id: Option<String> = conn
        .query_row(
            "SELECT peer_id FROM messages \
             WHERE smid = ?1 AND direction = 'sent' LIMIT 1",
            params![smid],
            |row| row.get(0),
        )
        .ok();
    if peer_id.is_some() {
        let _ = conn.execute(
            "UPDATE messages SET status = 'delivered' WHERE smid = ?1",
            params![smid],
        );
    }
    peer_id
}

// ── Reads ─────────────────────────────────────────────────────────────────────

pub struct RawMessage {
    pub db_id: i64,
    pub direction: String,
    pub ts: i64,
    pub nonce: Vec<u8>,
    pub ct: Vec<u8>,
    pub status: String,
    pub smid: Option<i64>,
    pub reply_to_ts: Option<i64>,
    pub reply_to_from: Option<String>,
    pub reply_to_text: Option<String>,
    pub file_id: Option<String>,
    pub file_name: Option<String>,
    pub file_mime: Option<String>,
    pub file_size: Option<i64>,
    pub group_id: Option<String>,
    pub sender_id: Option<String>,
    pub thumb_data: Option<Vec<u8>>,
    pub mentions: Option<String>,
    pub thread_parent_ts: Option<i64>,
    pub thread_parent_from: Option<String>,
    pub thread_reply_count: i64,
}

/// Return the highest smid stored for sent messages.
/// Used at startup to initialise the session counter above any previous value.
pub fn max_smid(conn: &Connection) -> u32 {
    conn.query_row(
        "SELECT COALESCE(MAX(smid), 0) FROM messages WHERE direction = 'sent'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0) as u32
}

/// Mark all sent messages to `peer_id` as read (called when peer sends a read receipt).
pub fn set_all_read(conn: &Connection, peer_id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE messages SET status = 'read' \
         WHERE peer_id = ?1 AND direction = 'sent' AND status != 'read'",
        params![peer_id],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Load message history for a peer with cursor-based pagination.
///
/// Returns at most `limit` messages older than `before_id` (exclusive), in
/// chronological (ASC) order. Pass `before_id = None` to start from the newest.
pub fn load_for_peer(
    conn: &Connection,
    peer_id: &str,
    limit: u32,
    before_id: Option<i64>,
) -> Result<Vec<RawMessage>, String> {
    let ceiling = before_id.unwrap_or(i64::MAX);
    let mut stmt = conn
        .prepare(
            "SELECT id, direction, ts, nonce, ct, status, smid, \
                    reply_to_ts, reply_to_from, reply_to_text, \
                    file_id, file_name, file_mime, file_size, \
                    group_id, sender_id, thumb_data, mentions, \
                    thread_parent_ts, thread_parent_from, \
                    (SELECT COUNT(*) FROM messages m2 \
                     WHERE m2.peer_id = messages.peer_id \
                       AND m2.thread_parent_ts = messages.ts) AS thread_reply_count \
             FROM messages WHERE peer_id = ?1 AND id < ?2 \
             ORDER BY id DESC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![peer_id, ceiling, limit], |row| {
            Ok(RawMessage {
                db_id:              row.get(0)?,
                direction:          row.get(1)?,
                ts:                 row.get(2)?,
                nonce:              row.get(3)?,
                ct:                 row.get(4)?,
                status:             row.get(5)?,
                smid:               row.get(6)?,
                reply_to_ts:        row.get(7)?,
                reply_to_from:      row.get(8)?,
                reply_to_text:      row.get(9)?,
                file_id:            row.get(10)?,
                file_name:          row.get(11)?,
                file_mime:          row.get(12)?,
                file_size:          row.get(13)?,
                group_id:           row.get(14)?,
                sender_id:          row.get(15)?,
                thumb_data:         row.get(16)?,
                mentions:           row.get(17)?,
                thread_parent_ts:   row.get(18)?,
                thread_parent_from: row.get(19)?,
                thread_reply_count: row.get(20).unwrap_or(0),
            })
        })
        .map_err(|e| e.to_string())?;

    let mut result = rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    result.reverse(); // return in chronological (ASC) order
    Ok(result)
}

/// Insert a sent group message. `peer_id` is set to `group_id` so that
/// `load_for_peer(group_id)` retrieves the full group history.
pub fn insert_group_sent(
    conn: &Connection,
    group_id: &str,
    sender_id: &str,
    ts: i64,
    nonce: &[u8; 12],
    ct: &[u8],
    reply_to_ts: Option<i64>,
    reply_to_from: Option<&str>,
    reply_to_text: Option<&str>,
    file_id: Option<&str>,
    file_name: Option<&str>,
    file_mime: Option<&str>,
    file_size: Option<i64>,
    thumb_data: Option<&[u8]>,
    thread_parent_ts: Option<i64>,
    thread_parent_from: Option<&str>,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO messages \
         (peer_id, direction, ts, nonce, ct, status, \
          reply_to_ts, reply_to_from, reply_to_text, \
          group_id, sender_id, \
          file_id, file_name, file_mime, file_size, thumb_data, \
          thread_parent_ts, thread_parent_from) \
         VALUES (?1, 'sent', ?2, ?3, ?4, 'sent', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![group_id, ts, nonce.as_slice(), ct,
                reply_to_ts, reply_to_from, reply_to_text,
                group_id, sender_id,
                file_id, file_name, file_mime, file_size, thumb_data,
                thread_parent_ts, thread_parent_from],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}


// ── Full-text search (in-memory) ──────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct SearchHit {
    pub db_id: i64,
    pub peer_id: String,
    pub group_id: Option<String>,
    pub sender_id: Option<String>,
    pub ts: i64,
    pub direction: String,
    pub snippet: String,
}

/// Create an in-memory SQLite FTS5 database. Called at unlock, dropped at lock.
pub fn create_fts_db() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory sqlite");
    conn.execute_batch(
        "CREATE VIRTUAL TABLE messages_fts USING fts5(
            content,
            content='',
            tokenize='unicode61 remove_diacritics 2'
        );",
    )
    .expect("create fts5 table");
    conn
}

/// Insert a plaintext message into the in-memory FTS index.
pub fn fts_insert(fts: &rusqlite::Connection, rowid: i64, plain: &str) {
    let _ = fts.execute(
        "INSERT INTO messages_fts(rowid, content) VALUES (?, ?)",
        rusqlite::params![rowid, plain],
    );
}

/// Remove a message from the in-memory FTS index.
pub fn fts_delete(fts: &rusqlite::Connection, rowid: i64) {
    let _ = fts.execute(
        "INSERT INTO messages_fts(messages_fts, rowid, content) VALUES ('delete', ?, '')",
        rusqlite::params![rowid],
    );
}

/// Search the in-memory FTS index. Returns (rowid, snippet) pairs.
pub fn fts_search(fts: &rusqlite::Connection, query: &str, limit: u32) -> Vec<(i64, String)> {
    let fts_query: String = if query.contains('"') {
        query.to_string()
    } else {
        query.split_whitespace()
            .map(|w| if w.ends_with('*') { w.to_string() } else { format!("{w}*") })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut stmt = match fts.prepare(
        "SELECT rowid, snippet(messages_fts, 0, '<<', '>>', '…', 8) \
         FROM messages_fts WHERE content MATCH ? ORDER BY rank LIMIT ?",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    stmt.query_map(rusqlite::params![fts_query, limit], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })
    .map(|it| it.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Decrypt and index all messages from the main DB into the in-memory FTS at unlock.
pub fn build_fts_index(
    db: &Connection,
    fts: &rusqlite::Connection,
    session_key: &[u8; 32],
) -> rusqlite::Result<()> {
    use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit};
    let cipher = ChaCha20Poly1305::new(session_key.into());
    let mut stmt = db.prepare(
        "SELECT id, nonce, ct FROM messages \
         WHERE file_id IS NULL ORDER BY ts DESC LIMIT 10000",
    )?;
    let rows: Vec<(i64, Vec<u8>, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    for (id, nonce, ct) in rows {
        if nonce.len() != 12 { continue; }
        let nonce_arr: [u8; 12] = nonce.try_into().unwrap();
        if let Ok(plain_bytes) = cipher.decrypt((&nonce_arr).into(), ct.as_slice()) {
            if let Ok(plain) = String::from_utf8(plain_bytes) {
                fts_insert(fts, id, &plain);
            }
        }
    }
    Ok(())
}

/// Fetch message metadata from the main DB to build a SearchHit for a given rowid.
pub fn get_message_meta(conn: &Connection, rowid: i64, snippet: String) -> Option<SearchHit> {
    conn.query_row(
        "SELECT id, peer_id, group_id, sender_id, ts, direction \
         FROM messages WHERE id = ?1",
        params![rowid],
        |row| {
            Ok(SearchHit {
                db_id:     row.get(0)?,
                peer_id:   row.get(1)?,
                group_id:  row.get(2)?,
                sender_id: row.get(3)?,
                ts:        row.get(4)?,
                direction: row.get(5)?,
                snippet,
            })
        },
    )
    .ok()
}

// ── Conversation read-state ───────────────────────────────────────────────────

/// Record that the user has read all messages in a conversation up to `ts`.
pub fn set_last_read(conn: &Connection, peer_id: &str, ts: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conv_meta (peer_id, last_read_ts) VALUES (?1, ?2) \
         ON CONFLICT(peer_id) DO UPDATE SET last_read_ts = excluded.last_read_ts \
         WHERE excluded.last_read_ts > conv_meta.last_read_ts",
        params![peer_id, ts],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Return the timestamp of the last read event for a conversation, or 0 if never read.
pub fn get_last_read(conn: &Connection, peer_id: &str) -> i64 {
    conn.query_row(
        "SELECT last_read_ts FROM conv_meta WHERE peer_id = ?1",
        params![peer_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Count unread received messages per conversation (ts > last_read_ts).
/// Returns a map of peer_id → unread count for all conversations with at least one unread.
pub fn get_all_unread_counts(conn: &Connection) -> std::collections::HashMap<String, i64> {
    conn.prepare(
        "SELECT m.peer_id, COUNT(*) \
         FROM messages m \
         LEFT JOIN conv_meta c ON c.peer_id = m.peer_id \
         WHERE m.direction = 'received' \
           AND m.ts > COALESCE(c.last_read_ts, 0) \
         GROUP BY m.peer_id",
    )
    .map(|mut s| {
        s.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

/// Insert a received group message. Uses INSERT OR IGNORE for duplicate safety.
/// Returns `true` if the row was inserted.
pub fn insert_group_received(
    conn: &Connection,
    group_id: &str,
    sender_id: &str,
    ts: i64,
    nonce: &[u8; 12],
    ct: &[u8],
    wire_hash: Option<&[u8; 32]>,
    reply_to_ts: Option<i64>,
    reply_to_from: Option<&str>,
    reply_to_text: Option<&str>,
    file_id: Option<&str>,
    file_name: Option<&str>,
    file_mime: Option<&str>,
    file_size: Option<i64>,
    thumb_data: Option<&[u8]>,
    mentions: Option<&str>,
    thread_parent_ts: Option<i64>,
    thread_parent_from: Option<&str>,
) -> Result<Option<i64>, String> {
    let rows = conn
        .execute(
            "INSERT OR IGNORE INTO messages \
             (peer_id, direction, ts, nonce, ct, status, msg_hash, \
              reply_to_ts, reply_to_from, reply_to_text, \
              group_id, sender_id, \
              file_id, file_name, file_mime, file_size, thumb_data, mentions, \
              thread_parent_ts, thread_parent_from) \
             VALUES (?1, 'received', ?2, ?3, ?4, 'delivered', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![group_id, ts, nonce.as_slice(), ct,
                    wire_hash.map(|h| h.as_slice()),
                    reply_to_ts, reply_to_from, reply_to_text,
                    group_id, sender_id,
                    file_id, file_name, file_mime, file_size, thumb_data, mentions,
                    thread_parent_ts, thread_parent_from],
        )
        .map_err(|e| e.to_string())?;
    if rows > 0 { Ok(Some(conn.last_insert_rowid())) } else { Ok(None) }
}

// ── Threads ───────────────────────────────────────────────────────────────────

/// All replies in a thread (messages whose thread_parent_ts == parent_ts in the given group).
pub fn get_thread_messages(
    conn: &Connection,
    group_id: &str,
    parent_ts: i64,
) -> Result<Vec<RawMessage>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, direction, ts, nonce, ct, status, smid, \
                    reply_to_ts, reply_to_from, reply_to_text, \
                    file_id, file_name, file_mime, file_size, \
                    group_id, sender_id, thumb_data, mentions, \
                    thread_parent_ts, thread_parent_from \
             FROM messages WHERE peer_id = ?1 AND thread_parent_ts = ?2 \
             ORDER BY id ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![group_id, parent_ts], |row| {
            Ok(RawMessage {
                db_id:              row.get(0)?,
                direction:          row.get(1)?,
                ts:                 row.get(2)?,
                nonce:              row.get(3)?,
                ct:                 row.get(4)?,
                status:             row.get(5)?,
                smid:               row.get(6)?,
                reply_to_ts:        row.get(7)?,
                reply_to_from:      row.get(8)?,
                reply_to_text:      row.get(9)?,
                file_id:            row.get(10)?,
                file_name:          row.get(11)?,
                file_mime:          row.get(12)?,
                file_size:          row.get(13)?,
                group_id:           row.get(14)?,
                sender_id:          row.get(15)?,
                thumb_data:         row.get(16)?,
                mentions:           row.get(17)?,
                thread_parent_ts:   row.get(18)?,
                thread_parent_from: row.get(19)?,
                thread_reply_count: 0,
            })
        })
        .map_err(|e| e.to_string())?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Count replies in a thread.
pub fn get_thread_reply_count(conn: &Connection, group_id: &str, parent_ts: i64) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE peer_id = ?1 AND thread_parent_ts = ?2",
        params![group_id, parent_ts],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
}

// ── Reactions ─────────────────────────────────────────────────────────────────

/// A single reaction row as returned to the frontend.
#[derive(serde::Serialize, Clone)]
pub struct ReactionRow {
    pub peer_id:    String,
    pub msg_ts:     i64,
    pub msg_from:   String,
    pub reactor_id: String,
    pub emoji:      String,
}

/// Upsert a reaction. Replaces any existing emoji from the same reactor on the
/// same message (one reactor = one reaction per message).
pub fn add_reaction(
    conn: &Connection,
    peer_id: &str,
    msg_ts: i64,
    msg_from: &str,
    reactor_id: &str,
    emoji: &str,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO reactions (peer_id, msg_ts, msg_from, reactor_id, emoji) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(peer_id, msg_ts, msg_from, reactor_id) \
         DO UPDATE SET emoji = excluded.emoji",
        params![peer_id, msg_ts, msg_from, reactor_id, emoji],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Remove a specific reactor's reaction from a message.
pub fn remove_reaction(
    conn: &Connection,
    peer_id: &str,
    msg_ts: i64,
    msg_from: &str,
    reactor_id: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM reactions WHERE peer_id=?1 AND msg_ts=?2 AND msg_from=?3 AND reactor_id=?4",
        params![peer_id, msg_ts, msg_from, reactor_id],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Load all reactions for a conversation (peer_id can be a DM or group_id).
pub fn get_reactions(conn: &Connection, peer_id: &str) -> Vec<ReactionRow> {
    conn.prepare(
        "SELECT peer_id, msg_ts, msg_from, reactor_id, emoji \
         FROM reactions WHERE peer_id = ?1",
    )
    .map(|mut s| {
        s.query_map(params![peer_id], |row| {
            Ok(ReactionRow {
                peer_id:    row.get(0)?,
                msg_ts:     row.get(1)?,
                msg_from:   row.get(2)?,
                reactor_id: row.get(3)?,
                emoji:      row.get(4)?,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    })
    .unwrap_or_default()
}

// ── Draft autosave ────────────────────────────────────────────────────────────

pub fn set_draft(conn: &Connection, peer_id: &str, text: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conv_meta (peer_id, draft) VALUES (?1, ?2) \
         ON CONFLICT(peer_id) DO UPDATE SET draft = excluded.draft",
        params![peer_id, text],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

pub fn get_draft(conn: &Connection, peer_id: &str) -> String {
    conn.query_row(
        "SELECT draft FROM conv_meta WHERE peer_id = ?1",
        params![peer_id],
        |row| row.get(0),
    )
    .unwrap_or_default()
}

// ── Mute / notification settings ─────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct MuteSettings {
    pub notifications_enabled: bool,
    pub mute_until: i64,
    pub is_muted: bool,
}

pub fn set_mute(
    conn: &Connection,
    peer_id: &str,
    notifications_enabled: bool,
    mute_until: i64,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conv_meta (peer_id, notifications_enabled, mute_until) \
         VALUES (?1, ?2, ?3) \
         ON CONFLICT(peer_id) DO UPDATE SET \
             notifications_enabled = excluded.notifications_enabled, \
             mute_until = excluded.mute_until",
        params![peer_id, notifications_enabled as i32, mute_until],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

pub fn get_mute(conn: &Connection, peer_id: &str, now_ms: i64) -> MuteSettings {
    let (enabled, mute_until): (i32, i64) = conn
        .query_row(
            "SELECT notifications_enabled, mute_until FROM conv_meta WHERE peer_id = ?1",
            params![peer_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap_or((1, 0));
    let notifications_enabled = enabled != 0;
    let is_muted = !notifications_enabled || (mute_until > 0 && mute_until > now_ms);
    MuteSettings { notifications_enabled, mute_until, is_muted }
}

// ── TTL / disappearing messages ───────────────────────────────────────────────

pub fn set_ttl(conn: &Connection, peer_id: &str, ttl_seconds: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO conv_meta (peer_id, ttl_seconds) VALUES (?1, ?2) \
         ON CONFLICT(peer_id) DO UPDATE SET ttl_seconds = excluded.ttl_seconds",
        params![peer_id, ttl_seconds],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

pub fn get_ttl(conn: &Connection, peer_id: &str) -> i64 {
    conn.query_row(
        "SELECT ttl_seconds FROM conv_meta WHERE peer_id = ?1",
        params![peer_id],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Delete expired messages from all conversations that have a TTL set.
/// Returns the number of messages deleted.
pub fn purge_expired(conn: &Connection, now_ms: i64) -> usize {
    conn.execute(
        "DELETE FROM messages \
         WHERE id IN ( \
             SELECT m.id FROM messages m \
             JOIN conv_meta c ON c.peer_id = m.peer_id \
             WHERE c.ttl_seconds > 0 \
               AND m.ts < (?1 - c.ttl_seconds * 1000) \
         )",
        params![now_ms],
    )
    .unwrap_or(0)
}

// ── Message editing ───────────────────────────────────────────────────────────


#[derive(serde::Serialize)]
pub struct EditHistoryEntry {
    pub old_plain: String,
    pub edited_at: i64,
}

/// Delete a message from the local DB by peer_id + timestamp.
/// Returns the db row id if found, None if not found.
/// FTS index is updated automatically via the messages_ad trigger.
pub fn delete_message(conn: &Connection, peer_id: &str, msg_ts: i64) -> Result<Option<i64>, String> {
    let msg_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM messages WHERE peer_id = ?1 AND ts = ?2 LIMIT 1",
            params![peer_id, msg_ts],
            |row| row.get(0),
        )
        .ok();
    let Some(msg_id) = msg_id else { return Ok(None) };
    conn.execute("DELETE FROM messages WHERE id = ?1", params![msg_id])
        .map_err(|e| e.to_string())?;
    Ok(Some(msg_id))
}

/// Apply an edit to a local message. Saves old plain to message_edits, then
/// updates messages.plain (FTS trigger fires automatically via messages_au).
pub fn apply_edit(
    conn: &Connection,
    peer_id: &str,
    msg_ts: i64,
    new_plain: &str,
    now: i64,
) -> Result<Option<i64>, String> {
    // Find the message.
    let msg_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM messages WHERE peer_id = ?1 AND ts = ?2 LIMIT 1",
            params![peer_id, msg_ts],
            |row| row.get(0),
        )
        .ok();
    let Some(msg_id) = msg_id else { return Ok(None) };

    // Save old version.
    let old_plain: String = conn
        .query_row(
            "SELECT COALESCE(plain, '') FROM messages WHERE id = ?1",
            params![msg_id],
            |row| row.get(0),
        )
        .unwrap_or_default();

    conn.execute(
        "INSERT OR IGNORE INTO message_edits (msg_id, old_plain, edited_at) VALUES (?1, ?2, ?3)",
        params![msg_id, old_plain, now],
    )
    .map_err(|e| e.to_string())?;

    // Update message (FTS auto-updated via trigger).
    conn.execute(
        "UPDATE messages SET plain = ?1, edited_at = ?2, edit_count = edit_count + 1 \
         WHERE id = ?3",
        params![new_plain, now, msg_id],
    )
    .map_err(|e| e.to_string())?;

    Ok(Some(msg_id))
}

pub fn get_edit_history(conn: &Connection, msg_id: i64) -> Vec<EditHistoryEntry> {
    conn.prepare(
        "SELECT old_plain, edited_at FROM message_edits \
         WHERE msg_id = ?1 ORDER BY edited_at ASC",
    )
    .map(|mut s| {
        s.query_map(params![msg_id], |row| {
            Ok(EditHistoryEntry { old_plain: row.get(0)?, edited_at: row.get(1)? })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    })
    .unwrap_or_default()
}

// ── Chat export ───────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
pub struct ExportMessage {
    pub ts: i64,
    pub direction: String,
    pub plain: String,
    pub sender_id: Option<String>,
    pub group_id: Option<String>,
    pub file_name: Option<String>,
    pub file_size: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct ExportReaction {
    pub msg_ts: i64,
    pub reactor_id: String,
    pub emoji: String,
}

pub fn load_for_export(conn: &Connection, peer_id: &str) -> (Vec<ExportMessage>, Vec<ExportReaction>) {
    let messages = conn
        .prepare(
            "SELECT ts, direction, COALESCE(plain, ''), sender_id, group_id, file_name, file_size \
             FROM messages WHERE peer_id = ?1 ORDER BY ts ASC",
        )
        .map(|mut s| {
            s.query_map(params![peer_id], |row| {
                Ok(ExportMessage {
                    ts:        row.get(0)?,
                    direction: row.get(1)?,
                    plain:     row.get(2)?,
                    sender_id: row.get(3)?,
                    group_id:  row.get(4)?,
                    file_name: row.get(5)?,
                    file_size: row.get(6)?,
                })
            })
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        })
        .unwrap_or_default();

    let reactions = conn
        .prepare(
            "SELECT msg_ts, reactor_id, emoji FROM reactions WHERE peer_id = ?1",
        )
        .map(|mut s| {
            s.query_map(params![peer_id], |row| {
                Ok(ExportReaction { msg_ts: row.get(0)?, reactor_id: row.get(1)?, emoji: row.get(2)? })
            })
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        })
        .unwrap_or_default();

    (messages, reactions)
}

// ── Sender Keys ───────────────────────────────────────────────────────────────

/// Store or update a sender chain key for (group_id, user_id).
pub fn set_sender_chain(
    conn: &Connection,
    group_id: &str,
    user_id: &str,
    chain_key: &[u8; 32],
    counter: u32,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO sender_keys (group_id, user_id, chain_key, counter) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(group_id, user_id) DO UPDATE SET chain_key=excluded.chain_key, counter=excluded.counter",
        params![group_id, user_id, chain_key.as_slice(), counter],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Retrieve the chain key, current counter, and distribution status for (group_id, user_id).
/// Returns `None` if no entry exists.
pub fn get_sender_chain(
    conn: &Connection,
    group_id: &str,
    user_id: &str,
) -> Option<([u8; 32], u32, bool)> {
    conn.query_row(
        "SELECT chain_key, counter, distributed FROM sender_keys WHERE group_id = ?1 AND user_id = ?2",
        params![group_id, user_id],
        |row| {
            let key_bytes: Vec<u8> = row.get(0)?;
            let counter: u32 = row.get(1)?;
            let distributed: bool = row.get(2)?;
            Ok((key_bytes, counter, distributed))
        },
    )
    .ok()
    .and_then(|(bytes, counter, distributed)| {
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some((arr, counter, distributed))
    })
}

/// Mark the sender chain as fully distributed to all group members.
/// Called by `distribute_sender_key` after all DR fan-out messages are sent.
pub fn mark_sk_distributed(conn: &Connection, group_id: &str, user_id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE sender_keys SET distributed = 1 WHERE group_id = ?1 AND user_id = ?2",
        params![group_id, user_id],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Atomically increment the send counter and return the old value (used for the next encrypt).
pub fn next_send_counter(conn: &Connection, group_id: &str, user_id: &str) -> Option<u32> {
    conn.query_row(
        "UPDATE sender_keys SET counter = counter + 1 \
         WHERE group_id = ?1 AND user_id = ?2 \
         RETURNING counter - 1",
        params![group_id, user_id],
        |row| row.get::<_, u32>(0),
    )
    .ok()
}

/// Delete the sender chain for a specific (group_id, user_id) pair.
/// Called when a member leaves so we stop accepting their group messages.
pub fn delete_sender_chain(conn: &Connection, group_id: &str, user_id: &str) {
    let _ = conn.execute(
        "DELETE FROM sender_keys WHERE group_id = ?1 AND user_id = ?2",
        params![group_id, user_id],
    );
}

/// Return all members of a group that have a sender chain stored (i.e., they distributed their key).
pub fn group_sk_members(conn: &Connection, group_id: &str) -> Vec<String> {
    conn.prepare(
        "SELECT user_id FROM sender_keys WHERE group_id = ?1",
    )
    .map(|mut s| {
        s.query_map(params![group_id], |r| r.get::<_, String>(0))
            .map(|it| it.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

// ── Group read marks ──────────────────────────────────────────────────────────

/// Upsert a per-member read watermark for a group. Only advances forward (MAX semantics).
pub fn set_group_read_mark(conn: &Connection, group_id: &str, user_id: &str, ts: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO group_read_marks (group_id, user_id, ts) VALUES (?1, ?2, ?3) \
         ON CONFLICT (group_id, user_id) DO UPDATE SET ts = MAX(excluded.ts, group_read_marks.ts)",
        params![group_id, user_id, ts],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Return `{ user_id → ts }` for all members who have sent a read receipt for a group.
pub fn get_group_read_marks(conn: &Connection, group_id: &str) -> std::collections::HashMap<String, i64> {
    conn.prepare("SELECT user_id, ts FROM group_read_marks WHERE group_id = ?1")
        .map(|mut s| {
            s.query_map(params![group_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map(|it| it.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

// ── Key Transparency state ────────────────────────────────────────────────────

/// Return `(last_id, last_hash)` for the given user, or `None` if not yet seen.
pub fn get_key_log_state(conn: &Connection, user_id: &str) -> Option<(i64, Vec<u8>)> {
    conn.query_row(
        "SELECT last_id, last_hash FROM key_log_state WHERE user_id = ?1",
        params![user_id],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
    )
    .ok()
    .and_then(|(id, hash)| Some((id, hash?)))
}

// ── Pinned messages ───────────────────────────────────────────────────────────

pub fn pin_message(
    conn: &Connection,
    peer_id: &str,
    msg_ts: i64,
    msg_from: &str,
    msg_text: &str,
    pinned_at: i64,
) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO pinned_messages (peer_id, msg_ts, msg_from, msg_text, pinned_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![peer_id, msg_ts, msg_from, msg_text, pinned_at],
    );
}

pub fn unpin_message(conn: &Connection, peer_id: &str, msg_ts: i64, msg_from: &str) {
    let _ = conn.execute(
        "DELETE FROM pinned_messages WHERE peer_id=?1 AND msg_ts=?2 AND msg_from=?3",
        params![peer_id, msg_ts, msg_from],
    );
}

#[derive(serde::Serialize)]
pub struct PinnedMsg {
    pub msg_ts:   i64,
    pub msg_from: String,
    pub msg_text: String,
    pub pinned_at: i64,
}

pub fn get_pinned_messages(conn: &Connection, peer_id: &str) -> Vec<PinnedMsg> {
    let mut stmt = match conn.prepare(
        "SELECT msg_ts, msg_from, msg_text, pinned_at \
         FROM pinned_messages WHERE peer_id=?1 ORDER BY pinned_at ASC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(params![peer_id], |row| {
        Ok(PinnedMsg {
            msg_ts:    row.get(0)?,
            msg_from:  row.get(1)?,
            msg_text:  row.get(2)?,
            pinned_at: row.get(3)?,
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    rows.flatten().collect()
}

// ── Saved messages ────────────────────────────────────────────────────────────

// ── Channels ──────────────────────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ChannelRow {
    pub channel_id:  String,
    pub group_id:    String,
    pub name:        String,
    pub description: Option<String>,
    pub subscribed:  bool,
}

pub fn upsert_channel(conn: &Connection, c: &ChannelRow) -> Result<(), String> {
    conn.execute(
        "INSERT INTO channels (channel_id, group_id, name, description, subscribed) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(channel_id) DO UPDATE SET \
             name = excluded.name, \
             description = excluded.description, \
             subscribed = excluded.subscribed",
        params![c.channel_id, c.group_id, c.name, c.description, c.subscribed as i32],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn get_channels_for_group(conn: &Connection, group_id: &str) -> Vec<ChannelRow> {
    conn.prepare(
        "SELECT channel_id, group_id, name, description, subscribed \
         FROM channels WHERE group_id = ?1 ORDER BY rowid",
    )
    .map(|mut s| {
        s.query_map(params![group_id], |row| {
            Ok(ChannelRow {
                channel_id:  row.get(0)?,
                group_id:    row.get(1)?,
                name:        row.get(2)?,
                description: row.get(3)?,
                subscribed:  row.get::<_, i32>(4)? != 0,
            })
        })
        .map(|it| it.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    })
    .unwrap_or_default()
}

pub fn delete_channel(conn: &Connection, channel_id: &str) {
    let _ = conn.execute("DELETE FROM channels WHERE channel_id = ?1", params![channel_id]);
}

pub const SAVED_PEER_ID: &str = "__saved__";

pub fn save_note(conn: &Connection, nonce: &[u8; 12], ct: &[u8], plain: &str, ts: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO messages (peer_id, direction, ts, nonce, ct, plain) \
         VALUES (?1, 'sent', ?2, ?3, ?4, ?5)",
        params![SAVED_PEER_ID, ts, nonce as &[u8], ct, plain],
    )?;
    Ok(())
}

// ── Scheduled messages ────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ScheduledMsg {
    pub id:               i64,
    pub peer_id:          String,
    pub is_group:         bool,
    pub is_channel:       bool,
    pub channel_group_id: Option<String>,
    pub text:             String,
    pub reply_to:         Option<String>,
    pub mentions:         Option<String>,
    pub send_at_ms:       i64,
    pub status:           String,
    pub error:            Option<String>,
    pub created_at:       i64,
}

pub fn insert_scheduled(
    conn: &Connection,
    peer_id: &str,
    is_group: bool,
    is_channel: bool,
    channel_group_id: Option<&str>,
    text: &str,
    reply_to: Option<&str>,
    mentions: Option<&str>,
    send_at_ms: i64,
) -> i64 {
    let now = crate::store::now_ms();
    let _ = conn.execute(
        "INSERT INTO scheduled_messages \
         (peer_id,is_group,is_channel,channel_group_id,text,reply_to,mentions,send_at_ms,created_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![peer_id, is_group as i64, is_channel as i64, channel_group_id,
                text, reply_to, mentions, send_at_ms, now],
    );
    conn.last_insert_rowid()
}

pub fn get_next_scheduled_time(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT MIN(send_at_ms) FROM scheduled_messages WHERE status='pending'",
        [],
        |r| r.get::<_, Option<i64>>(0),
    ).ok().flatten()
}

pub fn get_due_scheduled(conn: &Connection, now_ms: i64) -> Vec<ScheduledMsg> {
    let mut stmt = match conn.prepare(
        "SELECT id,peer_id,is_group,is_channel,channel_group_id,text,reply_to,mentions,\
                send_at_ms,status,error,created_at \
         FROM scheduled_messages WHERE status='pending' AND send_at_ms<=?1 ORDER BY send_at_ms"
    ) { Ok(s) => s, Err(_) => return vec![] };
    let x = match stmt.query_map(params![now_ms], row_to_scheduled) {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => vec![],
    }; x
}

pub fn get_scheduled_by_id(conn: &Connection, id: i64) -> Option<ScheduledMsg> {
    conn.query_row(
        "SELECT id,peer_id,is_group,is_channel,channel_group_id,text,reply_to,mentions,\
                send_at_ms,status,error,created_at \
         FROM scheduled_messages WHERE id=?1",
        params![id],
        row_to_scheduled,
    ).ok()
}

pub fn list_scheduled_for_peer(conn: &Connection, peer_id: &str) -> Vec<ScheduledMsg> {
    let mut stmt = match conn.prepare(
        "SELECT id,peer_id,is_group,is_channel,channel_group_id,text,reply_to,mentions,\
                send_at_ms,status,error,created_at \
         FROM scheduled_messages WHERE status='pending' AND peer_id=?1 ORDER BY send_at_ms"
    ) { Ok(s) => s, Err(_) => return vec![] };
    let x = match stmt.query_map(params![peer_id], row_to_scheduled) {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => vec![],
    }; x
}

pub fn mark_scheduled_sent(conn: &Connection, id: i64) {
    let _ = conn.execute(
        "UPDATE scheduled_messages SET status='sent' WHERE id=?1",
        params![id],
    );
}

pub fn mark_scheduled_failed(conn: &Connection, id: i64, error: &str) {
    let truncated = if error.len() > 500 { &error[..500] } else { error };
    let _ = conn.execute(
        "UPDATE scheduled_messages SET status='failed', error=?1 WHERE id=?2",
        params![truncated, id],
    );
}

pub fn set_scheduled_cancelled(conn: &Connection, id: i64) {
    let _ = conn.execute(
        "UPDATE scheduled_messages SET status='cancelled' WHERE id=?1",
        params![id],
    );
}

pub fn retry_failed_scheduled(conn: &Connection, grace_ms: i64) {
    let cutoff = crate::store::now_ms() - grace_ms;
    let _ = conn.execute(
        "UPDATE scheduled_messages SET status='pending', error=NULL \
         WHERE status='failed' AND send_at_ms >= ?1",
        params![cutoff],
    );
}

pub fn cleanup_old_scheduled(conn: &Connection) {
    let cutoff = crate::store::now_ms() - 7 * 86_400_000;
    let _ = conn.execute(
        "DELETE FROM scheduled_messages \
         WHERE status IN ('sent','cancelled') AND created_at < ?1",
        params![cutoff],
    );
}

fn row_to_scheduled(r: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledMsg> {
    Ok(ScheduledMsg {
        id:               r.get(0)?,
        peer_id:          r.get(1)?,
        is_group:         r.get::<_, i64>(2)? != 0,
        is_channel:       r.get::<_, i64>(3)? != 0,
        channel_group_id: r.get(4)?,
        text:             r.get(5)?,
        reply_to:         r.get(6)?,
        mentions:         r.get(7)?,
        send_at_ms:       r.get(8)?,
        status:           r.get(9)?,
        error:            r.get(10)?,
        created_at:       r.get(11)?,
    })
}

// ── C3 Link Preview cache ─────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub domain: String,
}

pub fn get_cached_preview(conn: &Connection, url: &str) -> Option<LinkPreview> {
    let cutoff = crate::store::now_ms() - 86_400_000; // 24 h
    conn.query_row(
        "SELECT url,title,description,image_url,domain FROM link_previews WHERE url=?1 AND fetched_at>?2",
        params![url, cutoff],
        |r| Ok(LinkPreview {
            url:         r.get(0)?,
            title:       r.get(1)?,
            description: r.get(2)?,
            image_url:   r.get(3)?,
            domain:      r.get(4)?,
        }),
    ).ok()
}

pub fn upsert_preview(conn: &Connection, p: &LinkPreview) {
    let _ = conn.execute(
        "INSERT OR REPLACE INTO link_previews(url,title,description,image_url,domain,fetched_at) \
         VALUES(?1,?2,?3,?4,?5,?6)",
        params![p.url, p.title, p.description, p.image_url, p.domain, crate::store::now_ms()],
    );
}

// ── C15 Data Retention ────────────────────────────────────────────────────────

pub fn set_retention_count(conn: &Connection, peer_id: &str, count: i64) {
    let _ = conn.execute(
        "INSERT INTO conv_meta(peer_id,retention_count) VALUES(?1,?2) \
         ON CONFLICT(peer_id) DO UPDATE SET retention_count=excluded.retention_count",
        params![peer_id, count],
    );
}

pub fn get_retention_count(conn: &Connection, peer_id: &str) -> i64 {
    conn.query_row(
        "SELECT retention_count FROM conv_meta WHERE peer_id=?1",
        params![peer_id],
        |r| r.get(0),
    ).unwrap_or(0)
}

pub fn enforce_retention_count(conn: &Connection, peer_id: &str, count: i64) {
    if count <= 0 { return; }
    let _ = conn.execute(
        "DELETE FROM messages WHERE peer_id=?1 AND id NOT IN \
         (SELECT id FROM messages WHERE peer_id=?1 ORDER BY ts DESC LIMIT ?2)",
        params![peer_id, count],
    );
}

pub fn get_all_retention_peers(conn: &Connection) -> Vec<(String, i64)> {
    let mut stmt = match conn.prepare(
        "SELECT peer_id, retention_count FROM conv_meta WHERE retention_count > 0"
    ) { Ok(s) => s, Err(_) => return vec![] };
    let x = match stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?))) {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => vec![],
    }; x
}

/// Persist the latest verified key log cursor for `user_id`.
pub fn set_key_log_state(conn: &Connection, user_id: &str, last_id: i64, last_hash: &[u8]) {
    let _ = conn.execute(
        "INSERT INTO key_log_state (user_id, last_id, last_hash, updated_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (user_id) DO UPDATE SET \
             last_id = excluded.last_id, \
             last_hash = excluded.last_hash, \
             updated_at = excluded.updated_at",
        params![user_id, last_id, last_hash, crate::store::now_ms() / 1000],
    );
}
