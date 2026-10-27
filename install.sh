#!/usr/bin/env bash
# ============================================================
#  Arizona RP parser bot — установка на Ubuntu/Debian одной командой
#  Запуск:  bash install.sh
# ============================================================
set -u
cd "$(dirname "$0")"
APP_DIR="$(pwd)"
SERVICE=arizona-parser
BIN="$APP_DIR/arizona-parser"

say() { echo -e "\n\033[1;36m== $* ==\033[0m"; }
ok()  { echo -e "\033[1;32m   OK:\033[0m $*"; }
warn(){ echo -e "\033[1;33m   !!\033[0m $*"; }

say "1/7 Системные пакеты"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl wget gnupg unzip build-essential pkg-config libssl-dev \
    libnss3 libatk-bridge2.0-0 libatk1.0-0 libcups2 libdrm2 libxkbcommon0 libxcomposite1 \
    libxdamage1 libxfixes3 libxrandr2 libgbm1 libpango-1.0-0 libcairo2 libasound2t64 \
    fonts-liberation libu2f-udev libvulkan1 >/dev/null 2>&1 || \
apt-get install -y -qq libnss3 libatk-bridge2.0-0 libatk1.0-0 libcups2 libdrm2 libxkbcommon0 \
    libxcomposite1 libxdamage1 libxfixes3 libxrandr2 libgbm1 libpango-1.0-0 libcairo2 \
    libasound2 fonts-liberation >/dev/null 2>&1 || true
ok "пакеты установлены"

say "2/7 Google Chrome"
CHROME=""
for c in /usr/bin/google-chrome /usr/bin/google-chrome-stable /usr/bin/chromium /usr/bin/chromium-browser /snap/bin/chromium; do
    [ -x "$c" ] && CHROME="$c" && break
done
if [ -z "$CHROME" ]; then
    warn "Chrome/Chromium не найден — ставлю Google Chrome"
    TMP=$(mktemp -d)
    if wget -q -O "$TMP/chrome.deb" https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb; then
        apt-get install -y -qq "$TMP/chrome.deb" >/dev/null 2>&1 || \
            { dpkg -i "$TMP/chrome.deb" >/dev/null 2>&1; apt-get -f install -y -qq >/dev/null 2>&1; }
    fi
    rm -rf "$TMP"
    for c in /usr/bin/google-chrome /usr/bin/google-chrome-stable; do
        [ -x "$c" ] && CHROME="$c" && break
    done
fi
if [ -n "$CHROME" ]; then ok "Chrome: $CHROME"; else warn "Chrome не установился — установи вручную"; fi

say "3/7 Rust"
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
export PATH="$HOME/.cargo/bin:$PATH"
cargo --version >/dev/null 2>&1 && ok "cargo: $(cargo --version)" || { warn "Rust не установился"; exit 1; }

say "4/7 Сборка (5-10 минут первый раз)"
if cargo build --release; then
    cp -f target/release/arizona-parser "$BIN"
    chmod +x "$BIN"
    ok "бинарник: $BIN"
else
    warn "Сборка не удалась"; exit 1
fi

say "5/7 Настройки"
# HEADLESS=true для сервера, CHROME_PATH очищаем (авто-поиск)
if [ -f config.toml ]; then
    sed -i 's/^HEADLESS *=.*/HEADLESS = true/' config.toml
    sed -i 's|^CHROME_PATH *=.*|CHROME_PATH = ""|' config.toml
    ok "config.toml: HEADLESS=true, Chrome определяется автоматически"
fi
for f in secret.key users.json state.db; do
    [ -f "$f" ] && ok "есть $f" || warn "нет $f (создастся при первом запуске)"
done

say "6/7 Автозапуск"
if command -v systemctl >/dev/null 2>&1 && [ -d /etc/systemd/system ]; then
    cat > "/etc/systemd/system/$SERVICE.service" <<EOF
[Unit]
Description=Arizona RP forum parser bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$APP_DIR
User=root
Environment=RUST_LOG=info
ExecStart=$BIN
Restart=always
RestartSec=10
StandardOutput=append:$APP_DIR/bot.log
StandardError=append:$APP_DIR/bot.log

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable "$SERVICE" >/dev/null 2>&1
    systemctl restart "$SERVICE"
    sleep 3
    if systemctl is-active --quiet "$SERVICE"; then
        ok "сервис $SERVICE запущен и включён в автозапуск"
    else
        warn "сервис не поднялся, смотри: journalctl -u $SERVICE -n 50 --no-pager"
    fi
    echo
    echo "  Статус:   systemctl status $SERVICE"
    echo "  Логи:     tail -f $APP_DIR/bot.log"
    echo "  Стоп:     systemctl stop $SERVICE"
    echo "  Рестарт:  systemctl restart $SERVICE"
else
    warn "systemd нет — запускаю через nohup"
    pkill -f "$BIN" >/dev/null 2>&1
    nohup "$BIN" >> "$APP_DIR/bot.log" 2>&1 &
    ok "запущен в фоне, PID $!"
    echo "  Логи: tail -f $APP_DIR/bot.log"
fi

echo


say "7/7 Watchdog"
cat > "$APP_DIR/watchdog.sh" <<'WD'
#!/bin/bash
SERVICE=arizona-parser
DIR=__APP_DIR__
LOG=$DIR/watchdog.log
ts() { date '+%Y-%m-%d %H:%M:%S'; }
if ! systemctl is-active --quiet "$SERVICE"; then
    systemctl restart "$SERVICE"
    echo "$(ts) service inactive -> restarted" >> "$LOG"
    exit 0
fi
if ! pgrep -f "profiles/vk" >/dev/null 2>&1; then
    sleep 30
    if ! pgrep -f "profiles/vk" >/dev/null 2>&1; then
        systemctl restart "$SERVICE"
        echo "$(ts) chrome not found -> restarted" >> "$LOG"
    fi
fi
find "$DIR/profiles" -maxdepth 1 -type d -mtime +1 -exec rm -rf {} + 2>/dev/null
[ -f "$DIR/bot.log" ] && [ $(stat -c%s "$DIR/bot.log") -gt 52428800 ] && tail -n 2000 "$DIR/bot.log" > "$DIR/bot.log.tmp" && mv "$DIR/bot.log.tmp" "$DIR/bot.log"
WD
sed -i "s|__APP_DIR__|$APP_DIR|g" "$APP_DIR/watchdog.sh"
chmod +x "$APP_DIR/watchdog.sh"

cat > /etc/systemd/system/arizona-watchdog.service <<'WDS'
[Unit]
Description=Arizona parser watchdog
[Service]
Type=oneshot
ExecStart=__APP_DIR__/watchdog.sh
WDS
sed -i "s|__APP_DIR__|$APP_DIR|g" /etc/systemd/system/arizona-watchdog.service

cat > /etc/systemd/system/arizona-watchdog.timer <<'WDT'
[Unit]
Description=Run Arizona parser watchdog every minute
[Timer]
OnBootSec=2min
OnUnitActiveSec=1min
[Install]
WantedBy=timers.target
WDT
systemctl daemon-reload
systemctl enable --now arizona-watchdog.timer >/dev/null 2>&1 || true
ok "watchdog установлен"

echo -e "\033[1;32mБОТ РАБОТАЕТ 24/7.\033[0m"
echo "  Статус:   systemctl status arizona-parser"
echo "  Логи:     tail -f $APP_DIR/bot.log"
echo "  Watchdog: systemctl list-timers | grep arizona"
