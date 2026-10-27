#!/bin/bash
# Обновление бота из git: ./update.sh
set -e
cd "$(dirname "$0")"
echo "== git pull =="
git pull
echo "== сборка =="
source "$HOME/.cargo/env" 2>/dev/null || true
CARGO_BUILD_JOBS=1 cargo build --release
cp -f target/release/arizona-parser ./arizona-parser
echo "== перезапуск =="
systemctl restart arizona-parser
sleep 5
tail -n 8 bot.log
echo "ГОТОВО"
