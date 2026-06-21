#!/usr/bin/env bash
# Release build: viewer assets first, then the binary with them embedded.
# This is the only supported release path — a plain `cargo build --release`
# ships the placeholder page instead of the viewer.
#
# Usage: build-release.sh [cargo-features]   (default: "viewer")
# install.sh passes a feature list here (e.g. "viewer,llm-openai") chosen by
# the user before the build starts.
set -euo pipefail
cd "$(dirname "$0")/.."

FEATURES="${1:-viewer}"

pnpm -C viewer install --frozen-lockfile
pnpm -C viewer build
cargo build --release --features "$FEATURES"

echo
echo "Release binary: target/release/poneglyph (features: $FEATURES)"
