# messenger — development commands
# requires: just, docker, cargo, sqlx-cli

DATABASE_URL := "postgres://messenger:secret@localhost/messenger"
NATS_URL     := "nats://localhost:4222"

# Start PostgreSQL and NATS, then run the server
dev:
    docker compose up -d
    DATABASE_URL={{DATABASE_URL}} NATS_URL={{NATS_URL}} cargo run -p messenger-server

# Start infrastructure only (no server)
infra:
    docker compose up -d

# Stop infrastructure
down:
    docker compose down

# Run all tests
test:
    cargo test --workspace --locked

# Run tests with output
test-verbose:
    cargo test --workspace --locked -- --nocapture

# Run security audits
audit:
    cargo audit
    cargo deny check

# Run clippy
lint:
    cargo clippy --workspace --locked -- -D warnings

# Apply database migrations
db-migrate:
    sqlx migrate run --database-url {{DATABASE_URL}}

# Revert last migration
db-revert:
    sqlx migrate revert --database-url {{DATABASE_URL}}

# Prepare sqlx offline query data (run after changing queries)
db-prepare:
    cargo sqlx prepare --workspace --database-url {{DATABASE_URL}}

# Reset database (drop + recreate + migrate)
db-reset:
    sqlx database drop --database-url {{DATABASE_URL}} -y || true
    sqlx database create --database-url {{DATABASE_URL}}
    sqlx migrate run --database-url {{DATABASE_URL}}

# Build release binary
build:
    cargo build --release -p messenger-server

# Format code
fmt:
    cargo fmt --all

# ── TLS ───────────────────────────────────────────────────────────────────────

# Generate self-signed cert.pem + key.pem in the current directory
tls-self-signed:
    cargo run -p messenger-server --bin generate-cert

# Run server with auto-generated self-signed TLS (cert.pem/key.pem written to cwd)
tls-dev:
    DATABASE_URL={{DATABASE_URL}} NATS_URL={{NATS_URL}} TLS_SELF_SIGNED=1 \
    BIND_ADDR=0.0.0.0:3000 cargo run -p messenger-server

# Run server with production TLS (Let's Encrypt)
tls-prod:
    TLS_CERT_PATH=/etc/letsencrypt/live/messenger/fullchain.pem \
    TLS_KEY_PATH=/etc/letsencrypt/live/messenger/privkey.pem \
    DATABASE_URL={{DATABASE_URL}} NATS_URL={{NATS_URL}} \
    cargo run -p messenger-server --release

# ── CI ────────────────────────────────────────────────────────────────────────

# Full CI check (what CI runs)
ci:
    cargo fmt --all --check
    cargo clippy --workspace --locked -- -D warnings
    cargo test --workspace --locked
    cargo audit
    cargo deny check
