#!/bin/sh
# Build the web (wasm) version into web/pkg/.
# Requires: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli
set -e
cd "$(dirname "$0")/.."
cargo build --release --target wasm32-unknown-unknown
wasm-bindgen --target no-modules --no-typescript \
  --out-dir web/pkg target/wasm32-unknown-unknown/release/rustpal.wasm
echo "done: web/pkg/ — run 'python3 web/serve.py' and open http://127.0.0.1:8080/web/"
