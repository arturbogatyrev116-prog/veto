#!/bin/bash
# Veto backup restore — DESTRUCTIVE, use with care
#
# Usage: restore.sh <path/to/daily-YYYY-MM-DD_HHMM.dump.age> [target_db]
#
# Requires the age private key: /root/.veto_backup_key (kept off-server for security)
# Copy it back before restoring:  scp admin@home:/secure/veto_backup_key /root/.veto_backup_key

set -euo pipefail

ENC_FILE="${1:-}"
TARGET_DB="${2:-messenger}"
PRIVATE_KEY="/root/.veto_backup_key"
POSTGRES_USER="${POSTGRES_USER:-messenger}"

if [ -z "$ENC_FILE" ] || [ ! -f "$ENC_FILE" ]; then
    echo "Usage: $0 <path/to/backup.dump.age> [target_db]"
    echo "Available backups:"
    ls /opt/veto/backups/daily-*.dump.age 2>/dev/null || echo "  (none found)"
    exit 1
fi

if [ ! -f "$PRIVATE_KEY" ]; then
    echo "ERROR: age private key not found at $PRIVATE_KEY"
    echo "Copy it from your secure offline storage before restoring."
    exit 1
fi

echo ""
echo "  ╔══════════════════════════════════════════════════════╗"
echo "  ║  WARNING: This will OVERWRITE the database '$TARGET_DB'  ║"
echo "  ║  All current data will be lost. There is no undo.   ║"
echo "  ╚══════════════════════════════════════════════════════╝"
echo ""
read -r -p "Type 'yes' to continue: " confirm
[ "$confirm" != "yes" ] && echo "Aborted." && exit 0

# Safety snapshot of current state before overwrite
PRE_RESTORE="/opt/veto/backups/pre-restore-$(date '+%Y-%m-%d_%H%M').dump"
echo "Creating safety snapshot → $PRE_RESTORE"
docker compose -f /opt/veto/docker-compose.prod.yml exec -T postgres \
    pg_dump -Fc -U "$POSTGRES_USER" "$TARGET_DB" > "$PRE_RESTORE" || true

DUMP_FILE="${ENC_FILE%.age}"
echo "Decrypting $ENC_FILE..."
age --decrypt --identity "$PRIVATE_KEY" --output "$DUMP_FILE" "$ENC_FILE"

echo "Restoring to database '$TARGET_DB'..."
docker compose -f /opt/veto/docker-compose.prod.yml exec -T postgres \
    pg_restore -c -d "$TARGET_DB" -U "$POSTGRES_USER" < "$DUMP_FILE"

rm -f "$DUMP_FILE"
echo "Restore complete."
