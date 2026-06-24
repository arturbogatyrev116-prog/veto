#!/bin/bash
# Veto server health check — runs every 2 minutes via cron
# Alerts on status change (ok→fail or fail→ok) to avoid spam

HEALTH_URL="https://veto.mooo.com/health"
STATE_FILE="/tmp/veto_monitor_state"
ALERT_SCRIPT="$(dirname "$0")/alert.sh"
TIMEOUT=10

check_health() {
    local http_code
    http_code=$(curl -sf --max-time "$TIMEOUT" -o /dev/null -w "%{http_code}" "$HEALTH_URL" 2>/dev/null)
    [ "$http_code" = "200" ] && echo "ok" || echo "fail"
}

current=$(check_health)
previous=$(cat "$STATE_FILE" 2>/dev/null || echo "ok")

NO_TELEGRAM_FLAG="$(dirname "$0")/.no_telegram"

if [ "$current" != "$previous" ]; then
    echo "$current" > "$STATE_FILE"
    if [ ! -f "$NO_TELEGRAM_FLAG" ]; then
        if [ "$current" = "fail" ]; then
            "$ALERT_SCRIPT" "🔴 Veto DOWN — сервер не отвечает ($(date '+%H:%M:%S'))"
        else
            "$ALERT_SCRIPT" "🟢 Veto UP — сервер восстановлен ($(date '+%H:%M:%S'))"
        fi
    fi
fi
