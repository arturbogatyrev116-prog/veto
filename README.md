# Veto Messenger

End-to-end encrypted messenger with post-quantum key exchange, built on Rust + Tauri.

**v0.3.0** — [Releases](https://github.com/arturbogatyrev116-prog/veto/releases) · CI [![CI](https://github.com/arturbogatyrev116-prog/veto/actions/workflows/ci.yml/badge.svg)](https://github.com/arturbogatyrev116-prog/veto/actions/workflows/ci.yml)

---

## Security model

| Layer | Algorithm |
|-------|-----------|
| Key exchange | X3DH + ML-KEM-768 (X-Wing hybrid PQ) |
| Ongoing encryption | Double Ratchet + ChaCha20-Poly1305 |
| Signatures | Ed25519 |
| Key derivation | HKDF-SHA256 |
| Group messaging | Sender Keys (O(1) per member) |
| Local database | SQLCipher + Argon2id key derivation |
| Key transparency | Append-only SHA-256 hash-chain log, self-audited on every session setup |

The server is **zero-knowledge**: it never sees plaintext. Message keys are derived client-side. Groups use Sender Keys so the server cannot correlate group membership with message content.

---

## Features

### Messaging
- Direct messages and group chats (E2EE)
- Channels (broadcast, admin-only posting)
- Post-quantum X3DH handshake (ML-KEM-768 + X25519)
- File attachments up to 20 MB (encrypted, inline image previews)
- Voice messages (Web Audio API → E2EE)
- Video messages (circular, HTML canvas → E2EE)
- Stickers (E2EE payload)
- Group events with RSVP
- Polls with live results

### UX
- Message scheduling ("Send Later") — smart scheduler, cancel-safe
- Link previews (SSRF-protected, cached 24h, sequential fetch queue)
- Slash commands (`/schedule`, `/poll`, `/event`, `/export`)
- Syntax highlighting in code blocks
- Drag & drop file upload
- Reply threading
- Message editing with edit history
- Reactions (emoji picker)
- Disappearing messages (TTL per chat)
- Data retention policies (keep last N messages per chat)

### Privacy & Security
- Safety numbers (fingerprint verification)
- Screen protection (blur on focus loss)
- Sender Key rotation when a member leaves a group
- Key Transparency: detects server-side key substitution

### Infrastructure
- Typing indicators, online presence
- Read receipts (DM + per-member in groups)
- Full-text search (FTS5 with prefix support)
- Chat export (Markdown / HTML / JSON / encrypted CEXP)
- Backup / restore (encrypted `.mbak`)
- Device manager (list + revoke sessions)
- Auto-update (hourly check, signed)
- System tray, dark/light theme

### Accessibility
- ARIA labels on all icon buttons
- Focus trap in modals
- Keyboard navigation
- `prefers-reduced-motion` support

---

## Stack

**Server** (`messenger-server/`)
- Rust · Axum · SQLx · PostgreSQL 17 · NATS JetStream
- JWT auth, rate limiting via governor (10 req/s per IP)
- Deployed on VPS with Docker Compose + Let's Encrypt

**Client** (`messenger-app/`)
- Tauri 2 + Svelte 4
- SQLite via rusqlite + SQLCipher (bundled)
- `messenger-crypto`: local crate for all crypto primitives

**Crypto** (`messenger-crypto/`)
- Pure Rust, no OpenSSL
- `ml-kem`, `x25519-dalek`, `ed25519-dalek`, `chacha20poly1305`, `hkdf`, `sha2`, `argon2`

---

## Project layout

```
ashyck/
├── messenger-crypto/      # crypto primitives (X3DH, Double Ratchet, Sender Keys, X-Wing)
├── messenger-server/      # Axum HTTP + WebSocket server
│   ├── src/routes/        # auth, messages, groups, polls, channels, files, ws, ...
│   ├── migrations/        # 0001–0014 SQL migrations
│   └── tests/integration/
├── messenger-app/         # Tauri 2 desktop app
│   ├── src/               # Svelte frontend
│   │   └── lib/           # MessagePane, Sidebar, LinkPreviewCard, PollView, ...
│   └── src-tauri/         # Rust backend (commands, db, client, store)
├── admin-ui/              # Svelte admin panel (/admin/)
├── monitor/               # Server health check scripts (cron + Telegram)
├── backup/                # pg_dump + age encryption scripts
├── docker-compose.prod.yml
├── deny.toml              # cargo-deny config (licenses + advisories)
└── .cargo/audit.toml      # cargo-audit CVE ignore list
```

---

## Building

**Prerequisites:** Rust stable (1.80+), Node.js 20+, PostgreSQL 17, NATS 2+

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

**CI** runs on every push: `cargo test`, `cargo clippy -D warnings`, `cargo audit`, `cargo deny check`.

---

## Server deployment

Production: `veto.mooo.com`. Managed via desktop bat-scripts:

| Script | Action |
|--------|--------|
| `Veto - Запустить.bat` | `docker compose up -d` |
| `Veto - Остановить.bat` | `docker compose down` |
| `Veto - Пересобрать.bat` | build → `up -d` |
| `Veto - Статус.bat` | `ps` + health |
| `Veto - Обновить конфиг.bat` | validate env → `down` → `up --force-recreate` |

Monitoring: Uptime Kuma (Docker) + cron `check.sh` with Telegram alerts.
Backups: daily `pg_dump` encrypted with `age`, 7-day rotation, `restore.sh` with safety snapshot.

---

## License

MIT
