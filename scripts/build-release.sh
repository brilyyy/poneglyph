#!/usr/bin/env bash
# Release build: viewer assets first, then the binary with them embedded.
# This is the only supported release path — a plain `cargo build --release`
# ships the placeholder page instead of the viewer.
set -euo pipefail
cd "$(dirname "$0")/.."

pnpm -C viewer install --frozen-lockfile
pnpm -C viewer build
cargo build --release --features viewer

echo
echo "Release binary: target/release/poneglyph (viewer embedded)"
