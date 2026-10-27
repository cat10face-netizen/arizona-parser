#!/bin/bash
# Watchdog: перезапускает бота, если сервис неактивен или Chrome умер.
SERVICE=arizona-parser
DIR=/root/arizona-parser-portable
LOG=$DIR/watchdog.log

ts() { date '+%Y-%m-%d %H:%M:%S'; }

if ! systemctl is-active --quiet "$SERVICE"; then
    systemctl restart "$SERVICE"
    echo "$(ts) service inactive -> restarted" >> "$LOG"
    exit 0
fi

# Chrome для парсинга должен работать (профиль profiles/vk*)
if ! pgrep -f "profiles/vk" >/dev/null 2>&1; then
    # даём 30 секунд на старт, чтобы не дёргать при рестарте
    sleep 30
    if ! pgrep -f "profiles/vk" >/dev/null 2>&1; then
        systemctl restart "$SERVICE"
        echo "$(ts) chrome not found -> restarted" >> "$LOG"
    fi
fi

# чистим старые профили, чтобы не рос диск
find "$DIR/profiles" -maxdepth 1 -type d -mtime +1 -exec rm -rf {} + 2>/dev/null
# лог не должен расти бесконечно
if [ -f "$DIR/bot.log" ] && [ $(stat -c%s "$DIR/bot.log") -gt 52428800 ]; then
    tail -n 2000 "$DIR/bot.log" > "$DIR/bot.log.tmp" && mv "$DIR/bot.log.tmp" "$DIR/bot.log"
fi
