#!/bin/bash
cd "$(dirname "$0")"
cargo build --release || exit 1
echo "Готово: target/release/arizona-parser"
