#!/bin/bash
# Veto backup rotation — keeps last 7 days, minimum 3 files
# Safe: counts existing backups before deleting, never removes if < 3 remain

BACKUP_DIR="/opt/veto/backups"
KEEP_DAYS=7
MIN_KEEP=3
LOG="$BACKUP_DIR/backup.log"

log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*" | tee -a "$LOG"; }

total=$(find "$BACKUP_DIR" -name "daily-*.dump.age" | wc -l)
if [ "$total" -le "$MIN_KEEP" ]; then
    log "Rotation skipped: only $total backup(s) exist (minimum $MIN_KEEP)"
    exit 0
fi

deleted=0
while IFS= read -r f; do
    remaining=$((total - deleted))
    if [ "$remaining" -le "$MIN_KEEP" ]; then
        break
    fi
    log "Removing old backup: $(basename "$f")"
    rm -f "$f"
    deleted=$((deleted + 1))
done < <(find "$BACKUP_DIR" -name "daily-*.dump.age" -mtime +"$KEEP_DAYS" | sort)

log "Rotation complete: removed $deleted file(s), $((total - deleted)) remaining"
