#!/bin/bash
# Veto database backup — encrypts with age and stores locally
# Runs daily via cron; paired with rotate.sh for 7-day retention

set -euo pipefail

BACKUP_DIR="/opt/veto/backups"
KEY_FILE="/root/.veto_backup.pub"   # age public key (encrypt-only, safe on server)
LOG="$BACKUP_DIR/backup.log"
TIMESTAMP=$(date '+%Y-%m-%d_%H%M')
DUMP_FILE="$BACKUP_DIR/daily-${TIMESTAMP}.dump"
ENC_FILE="${DUMP_FILE}.age"

# Read DB credentials from docker-compose env
POSTGRES_USER="${POSTGRES_USER:-messenger}"
POSTGRES_DB="${POSTGRES_DB:-messenger}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-}"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG"; }

mkdir -p "$BACKUP_DIR"

if [ ! -f "$KEY_FILE" ]; then
    log "ERROR: age public key not found at $KEY_FILE — run setup_backup first"
    exit 1
fi

log "Starting backup: $DUMP_FILE"

# Dump from the running postgres container
docker compose -f /opt/veto/docker-compose.prod.yml exec -T postgres \
    pg_dump -Fc -Z 9 -U "$POSTGRES_USER" "$POSTGRES_DB" > "$DUMP_FILE"

log "Dump complete ($(du -sh "$DUMP_FILE" | cut -f1)), encrypting..."

age --encrypt --recipient "$(cat "$KEY_FILE")" --output "$ENC_FILE" "$DUMP_FILE"
rm -f "$DUMP_FILE"

log "Backup done: $ENC_FILE ($(du -sh "$ENC_FILE" | cut -f1))"
