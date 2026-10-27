#!/bin/bash
cd "$(dirname "$0")"
export RUST_LOG=info
BIN="./arizona-parser"
[ -x "$BIN" ] || BIN="./target/release/arizona-parser"
"$BIN"
echo
echo "Бот остановлен."
