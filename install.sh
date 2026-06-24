#!/usr/bin/env bash
set -euo pipefail

GREEN='\033[0;32m'; YELLOW='\033[1;33m'; RED='\033[0;31m'; NC='\033[0m'
info()  { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
error() { echo -e "${RED}[✗]${NC} $*" >&2; exit 1; }

# ── Prerequisites ─────────────────────────────────────────────────────────────

command -v docker  >/dev/null 2>&1 || error "Docker not found. Install from https://docs.docker.com/get-docker/"
command -v openssl >/dev/null 2>&1 || error "openssl not found. Install: apt-get install openssl"

# docker compose (v2) or docker-compose (v1)
if docker compose version >/dev/null 2>&1; then
    DC="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
    DC="docker-compose"
else
    error "Docker Compose not found. Install from https://docs.docker.com/compose/install/"
fi

# ── .env setup ────────────────────────────────────────────────────────────────

if [ ! -f .env ]; then
    if [ ! -f .env.example ]; then
        error ".env.example not found. Run this script from the repository root."
    fi
    cp .env.example .env
    warn ".env created from .env.example"
    warn "Edit .env and set POSTGRES_PASSWORD and ADMIN_TOKEN, then re-run this script."
    exit 1
fi

# Warn if placeholder values are still in .env
if grep -q "change_me_before_deploy" .env; then
    error "Update POSTGRES_PASSWORD and ADMIN_TOKEN in .env before deploying."
fi

# ── TLS certificates ──────────────────────────────────────────────────────────

mkdir -p certs

if [ -z "${TLS_CERT_PATH:-}" ] && [ -z "${TLS_SELF_SIGNED:-}" ]; then
    if [ ! -f certs/cert.pem ] || [ ! -f certs/key.pem ]; then
        info "Generating self-signed TLS certificate (valid 365 days)..."
        openssl req -x509 -newkey rsa:4096 \
            -keyout certs/key.pem -out certs/cert.pem \
            -days 365 -nodes \
            -subj "/CN=messenger-server" \
            -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" \
            2>/dev/null
        # Inject into .env so the server picks it up
        if ! grep -q "^TLS_SELF_SIGNED=" .env; then
            echo "TLS_SELF_SIGNED=1" >> .env
        fi
        warn "Self-signed certificate generated. Clients must accept it (testing only)."
        warn "For production, obtain a Let's Encrypt cert and set TLS_CERT_PATH / TLS_KEY_PATH in .env"
    fi
fi

# ── Build & start ─────────────────────────────────────────────────────────────

info "Building server image..."
$DC -f docker-compose.prod.yml build --pull

info "Starting services..."
$DC -f docker-compose.prod.yml up -d

info "Waiting for server to become healthy..."
for i in $(seq 1 30); do
    if curl -sk "https://localhost:${BIND_PORT:-3000}/health" >/dev/null 2>&1 || \
       curl -sk "http://localhost:${BIND_PORT:-3000}/health"  >/dev/null 2>&1; then
        echo ""
        info "Messenger Server is running!"
        info "Endpoint:   https://localhost:${BIND_PORT:-3000}"
        info "Admin API:  https://localhost:${BIND_PORT:-3000}/api/v1/admin/users"
        info "Logs:       $DC -f docker-compose.prod.yml logs -f server"
        exit 0
    fi
    printf "."
    sleep 2
done

echo ""
error "Server did not become healthy in 60 seconds. Check logs: $DC -f docker-compose.prod.yml logs server"
