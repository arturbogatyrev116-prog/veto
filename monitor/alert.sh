#!/bin/bash
# Send a Telegram alert. Called by check.sh with the message as $1.
# Credentials are injected by setup_monitor during installation.

TELEGRAM_TOKEN="__TELEGRAM_TOKEN__"
TELEGRAM_CHAT_ID="__TELEGRAM_CHAT_ID__"

MESSAGE="$1"
[ -z "$MESSAGE" ] && exit 1

curl -sf -X POST "https://api.telegram.org/bot${TELEGRAM_TOKEN}/sendMessage" \
    -d "chat_id=${TELEGRAM_CHAT_ID}" \
    -d "text=${MESSAGE}" \
    > /dev/null
