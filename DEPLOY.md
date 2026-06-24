# Veto Messenger — On-Premise Deployment Guide

## Requirements

- Linux server (Ubuntu 22.04+ recommended) or any Docker host
- Docker 24+ and Docker Compose v2
- 1 GB RAM minimum (2 GB recommended for 100+ users)
- 10 GB disk space
- Domain name (for production TLS) or static IP (for LAN)

## Quick Start

```bash
git clone <your-repo-url> veto-messenger
cd veto-messenger

# Edit configuration
cp .env.example .env
nano .env  # Set POSTGRES_PASSWORD and ADMIN_TOKEN

# Install and start
chmod +x install.sh
./install.sh
```

The server starts at `https://localhost:3000` (or your configured port).

---

## Configuration

All settings are in `.env`:

| Variable | Required | Description |
|----------|----------|-------------|
| `POSTGRES_PASSWORD` | ✅ | PostgreSQL password |
| `ADMIN_TOKEN` | ✅ | Bearer token for admin API |
| `POSTGRES_DB` | — | Database name (default: `messenger`) |
| `POSTGRES_USER` | — | DB user (default: `messenger`) |
| `BIND_PORT` | — | Host port (default: `3000`) |
| `TLS_CERT_PATH` | — | Path to TLS certificate inside container |
| `TLS_KEY_PATH` | — | Path to TLS private key inside container |
| `TLS_SELF_SIGNED` | — | Set to `1` to auto-generate self-signed cert |
| `CERTS_DIR` | — | Host directory mounted as `/certs` (default: `./certs`) |

---

## TLS Setup

### Option A: Let's Encrypt (production)

```bash
# Install certbot
sudo apt-get install certbot

# Obtain certificate (stop server first to free port 80)
sudo certbot certonly --standalone -d messenger.yourdomain.com

# Set in .env
CERTS_DIR=/etc/letsencrypt/live/messenger.yourdomain.com
TLS_CERT_PATH=/certs/fullchain.pem
TLS_KEY_PATH=/certs/privkey.pem

# Restart
docker compose -f docker-compose.prod.yml restart server
```

Auto-renew: `sudo certbot renew --pre-hook "docker compose ... stop server" --post-hook "docker compose ... start server"`

### Option B: Self-signed (LAN / testing)

```bash
# install.sh generates this automatically, or manually:
TLS_SELF_SIGNED=1  # in .env
```

Clients must accept the self-signed certificate (`MESSENGER_INSECURE_TLS=1` in client config).

---

## Admin API

All admin endpoints require `Authorization: Bearer <ADMIN_TOKEN>` header.

### List users
```bash
curl -sk https://localhost:3000/api/v1/admin/users \
  -H "Authorization: Bearer $ADMIN_TOKEN" | jq .
```

### Create user
```bash
curl -sk -X POST https://localhost:3000/api/v1/admin/users \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"username": "alice"}' | jq .
# Returns: { "user_id": "...", "username": "alice", "token": "<bearer_token>" }
# Give the token to the user — it is shown only once.
```

### Block user
```bash
curl -sk -X POST https://localhost:3000/api/v1/admin/users/<user_id>/block \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

### Unblock user
```bash
curl -sk -X POST https://localhost:3000/api/v1/admin/users/<user_id>/unblock \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

### Delete user
```bash
curl -sk -X DELETE https://localhost:3000/api/v1/admin/users/<user_id> \
  -H "Authorization: Bearer $ADMIN_TOKEN"
```

---

## Backup & Restore

### Backup database
```bash
docker compose -f docker-compose.prod.yml exec postgres \
  pg_dump -U messenger messenger > backup_$(date +%Y%m%d_%H%M%S).sql
```

### Restore database
```bash
cat backup.sql | docker compose -f docker-compose.prod.yml exec -T postgres \
  psql -U messenger messenger
```

---

## Operations

### View logs
```bash
docker compose -f docker-compose.prod.yml logs -f server
docker compose -f docker-compose.prod.yml logs -f postgres
```

### Stop / start
```bash
docker compose -f docker-compose.prod.yml down
docker compose -f docker-compose.prod.yml up -d
```

### Update server
```bash
git pull
docker compose -f docker-compose.prod.yml build --pull server
docker compose -f docker-compose.prod.yml up -d --no-deps server
```

### Health check
```bash
curl -sk https://localhost:3000/health
# Returns: {"status":"ok"}
```

---

## Client Configuration

Point the desktop client to your server:

**Windows:** create `server_url.txt` next to `Veto.exe`:
```
https://messenger.yourdomain.com:3000
```

**Environment variable** (any OS):
```bash
MESSENGER_SERVER_URL=https://messenger.yourdomain.com:3000 ./Veto
```

For self-signed certificates, also set:
```bash
MESSENGER_INSECURE_TLS=1
```

---

## Security Notes

- `ADMIN_TOKEN` grants full user management — treat it like a root password
- The server never stores plaintext messages (E2EE — server sees only encrypted blobs)
- File uploads are stored encrypted; only the recipient holds the decryption key
- Database contains user metadata and encrypted prekeys — protect with disk encryption
- Rotate `ADMIN_TOKEN` by updating `.env` and restarting the server
