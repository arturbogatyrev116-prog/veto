# Veto Messenger

End-to-end encrypted messenger with post-quantum key exchange, built on Rust + Tauri.

---

## Security model

| Layer | Algorithm |
|-------|-----------|
| Key exchange | X3DH + ML-KEM-768 (X-Wing hybrid) |
| Ongoing encryption | Double Ratchet + ChaCha20-Poly1305 |
| Signatures | Ed25519 |
| Key derivation | HKDF-SHA256 |
| Group messaging | Sender Keys (O(1) per member) |
| Local database | SQLCipher + Argon2id key derivation |
| Key transparency | Append-only SHA-256 hash-chain log, self-audited on every session setup |

The server is zero-knowledge: it never sees plaintext. Message keys are derived client-side, groups use Sender Keys so the server cannot correlate group membership with message content.

---

## Features

- Direct messages and group chats (E2EE)
- Post-quantum X3DH handshake (ML-KEM-768)
- File attachments up to 20 MB (encrypted, with inline image previews)
- Disappearing messages (24h / 7d / 30d)
- Message editing with edit history
- Reactions (emoji picker)
- Reply threading
- Read receipts (DM + per-member in groups)
- Sender Key rotation when a member leaves a group
- Typing indicators
- Full-text search (FTS5)
- Chat export (Markdown / HTML / JSON, optionally encrypted)
- Safety numbers (fingerprint verification)
- Backup / restore (encrypted `.mbak`)
- Device manager
- Key Transparency: detects server-side key substitution

---

## Stack

**Server** (`messenger-server/`)
- Rust · Axum · SQLx · PostgreSQL 17 · NATS JetStream
- JWT-less auth: 64-char hex bearer tokens
- Rate limiting: governor (10 req/s per IP)
- Deployed on VPS with Docker Compose + Let's Encrypt

**Client** (`messenger-app/`)
- Tauri 2 + Svelte
- SQLite (via rusqlite + SQLCipher)
- messenger-crypto: local crate implementing all crypto primitives

**Crypto** (`messenger-crypto/`)
- Pure Rust, no OpenSSL
- ml-kem, x25519-dalek, ed25519-dalek, chacha20poly1305, hkdf, sha2

---

## Project layout

```
ashyck/
├── messenger-crypto/      # crypto primitives (X3DH, Double Ratchet, Sender Keys)
├── messenger-server/      # Axum HTTP + WebSocket server
│   ├── src/routes/
│   └── tests/integration/
├── messenger-app/         # Tauri desktop app
│   ├── src/               # Svelte frontend
│   └── src-tauri/         # Rust backend (commands, db, client, crypto bridge)
├── admin-ui/              # Svelte admin panel
├── monitor/               # Server health check scripts (cron + Telegram)
├── backup/                # pg_dump + age encryption scripts
├── docker-compose.prod.yml
├── deny.toml              # cargo-deny config
└── .cargo/audit.toml      # cargo-audit CVE ignore list
```

---

## Building

**Prerequisites:** Rust stable, Node.js 20+, PostgreSQL 17, NATS 2.11+

```bash
# Server
cd messenger-server
sqlx migrate run
cargo run

# Desktop app
cd messenger-app
npm install
npm run tauri dev
```

**CI** runs `cargo test`, `cargo clippy`, `cargo audit`, and `cargo deny check` on every push.

---

## Server deployment

The production server runs at `veto.mooo.com`. Managed via bat-scripts on the developer's desktop:

| Script | Action |
|--------|--------|
| `Veto - Запустить.bat` | `docker compose up -d` |
| `Veto - Остановить.bat` | `docker compose down` |
| `Veto - Перезапустить.bat` | `down` → `up -d` |
| `Veto - Пересобрать.bat` | `pull` → `build` → `up -d` |
| `Veto - Статус.bat` | `ps` + container health |

Monitoring: Uptime Kuma (Docker) + cron `check.sh` with Telegram alerts.  
Backups: daily `pg_dump` encrypted with `age`, 7-day rotation.

---

## License

MIT
