# ── Build stage ────────────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /build

# Cache dependency compilation separately from source
COPY Cargo.toml Cargo.lock ./
COPY messenger-crypto/Cargo.toml messenger-crypto/Cargo.toml
COPY messenger-server/Cargo.toml messenger-server/Cargo.toml

# Drop messenger-app from workspace: it belongs to the desktop client, not the server.
# Keeping it would require pulling the full Tauri dependency tree into this image.
RUN sed -i '/messenger-app/d' Cargo.toml

# Stub source files so cargo can resolve the dependency graph
RUN mkdir -p messenger-crypto/src messenger-server/src \
    && echo "fn main() {}" > messenger-server/src/main.rs \
    && echo "" > messenger-crypto/src/lib.rs \
    && cargo build --release -p messenger-server \
    && rm -rf messenger-crypto/src messenger-server/src

# Build real source
COPY messenger-crypto messenger-crypto
COPY messenger-server messenger-server

RUN touch messenger-crypto/src/lib.rs messenger-server/src/main.rs \
    && cargo build --release -p messenger-server

# ── Runtime stage ──────────────────────────────────────────────────────────────
# ubuntu:24.04 ships GLIBC 2.39 — satisfies any rust:latest binary requirement.
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && ln -fs /usr/share/zoneinfo/UTC /etc/localtime

COPY --from=builder /build/target/release/messenger-server /usr/local/bin/messenger-server

EXPOSE 3000
ENV RUST_LOG=messenger_server=info,tower_http=warn

ENTRYPOINT ["/usr/local/bin/messenger-server"]
