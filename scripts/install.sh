#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# ── colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'; BOLD='\033[1m'; RESET='\033[0m'
ok()   { echo -e "${GREEN}✓${RESET} $*"; }
warn() { echo -e "${YELLOW}!${RESET} $*"; }
die()  { echo -e "${RED}✗${RESET} $*" >&2; exit 1; }
hdr()  { echo -e "\n${BOLD}$*${RESET}"; }

# ── defaults ──────────────────────────────────────────────────────────────────
SKIP_BUILD=false
SYSTEM=false
INSTALL_HOOKS=false
NO_INIT=false
YES=false

# ── args ──────────────────────────────────────────────────────────────────────
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=true ;;
    --system)     SYSTEM=true ;;
    --hooks)      INSTALL_HOOKS=true ;;
    --no-init)    NO_INIT=true ;;
    --yes)        YES=true ;;
    --help|-h)
      echo "Usage: $0 [--skip-build] [--system] [--hooks] [--no-init] [--yes]"
      echo ""
      echo "  --skip-build   Use existing target/release/poneglyph (skip build)"
      echo "  --system       Install to /usr/local/bin (requires sudo)"
      echo "  --hooks        Copy Claude Code hooks to ~/.config/poneglyph/hooks/"
      echo "  --no-init      Skip running 'poneglyph init' after install"
      echo "  --yes          Non-interactive, accept all defaults"
      exit 0
      ;;
    *) die "Unknown flag: $arg. Run with --help for usage." ;;
  esac
done

# ── prereqs ───────────────────────────────────────────────────────────────────
hdr "Checking prerequisites"

if ! command -v rustc &>/dev/null; then
  die "Rust not found. Install via https://rustup.rs"
fi
RUST_VERSION=$(rustc --version | grep -oP '\d+\.\d+' | head -1)
RUST_MAJOR=$(echo "$RUST_VERSION" | cut -d. -f1)
RUST_MINOR=$(echo "$RUST_VERSION" | cut -d. -f2)
if [[ "$RUST_MAJOR" -lt 1 || ( "$RUST_MAJOR" -eq 1 && "$RUST_MINOR" -lt 75 ) ]]; then
  die "Rust 1.75+ required (found $RUST_VERSION). Run: rustup update"
fi
ok "Rust $RUST_VERSION"

if ! command -v pnpm &>/dev/null; then
  if [[ "$SKIP_BUILD" == true ]]; then
    warn "pnpm not found (skipping build anyway)"
  else
    die "pnpm not found. Install via: npm i -g pnpm  or  https://pnpm.io/installation"
  fi
else
  ok "pnpm $(pnpm --version)"
fi

if ! command -v jq &>/dev/null;   then warn "jq not found — needed for Claude Code hooks"; fi
if ! command -v curl &>/dev/null; then warn "curl not found — needed for Claude Code hooks"; fi

# ── build ─────────────────────────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == false ]]; then
  hdr "Building poneglyph (viewer embedded)"
  bash scripts/build-release.sh
else
  hdr "Skipping build (--skip-build)"
fi

BINARY="target/release/poneglyph"
[[ -f "$BINARY" ]] || die "Binary not found at $BINARY. Run without --skip-build."
[[ -x "$BINARY" ]] || chmod +x "$BINARY"
ok "Binary ready: $BINARY"

# ── install binary ────────────────────────────────────────────────────────────
hdr "Installing binary"

if [[ "$SYSTEM" == true ]]; then
  INSTALL_DIR="/usr/local/bin"
  INSTALL_PATH="$INSTALL_DIR/poneglyph"
  echo "Installing to $INSTALL_PATH (requires sudo)"
  sudo install -m 755 "$BINARY" "$INSTALL_PATH"
else
  INSTALL_DIR="$HOME/.local/bin"
  INSTALL_PATH="$INSTALL_DIR/poneglyph"
  mkdir -p "$INSTALL_DIR"
  install -m 755 "$BINARY" "$INSTALL_PATH"
fi
ok "Installed: $INSTALL_PATH"

# PATH check
if ! echo ":$PATH:" | grep -q ":$INSTALL_DIR:"; then
  warn "$INSTALL_DIR is not on your \$PATH"
  warn "Add this to your shell rc: export PATH=\"\$HOME/.local/bin:\$PATH\""
fi

# ── hooks ─────────────────────────────────────────────────────────────────────
if [[ "$INSTALL_HOOKS" == false && "$YES" == false ]]; then
  hdr "Claude Code hooks"
  read -r -p "Install Claude Code hooks to ~/.config/poneglyph/hooks/? [y/N] " reply
  [[ "$reply" =~ ^[Yy]$ ]] && INSTALL_HOOKS=true
fi

if [[ "$INSTALL_HOOKS" == true ]]; then
  HOOKS_DEST="$HOME/.config/poneglyph/hooks"
  mkdir -p "$HOOKS_DEST"
  cp hooks/claude-code/posttooluse.sh    "$HOOKS_DEST/"
  cp hooks/claude-code/sessionstart.sh   "$HOOKS_DEST/"
  cp hooks/claude-code/stop.sh           "$HOOKS_DEST/"
  cp hooks/claude-code/userpromptsubmit.sh "$HOOKS_DEST/"
  chmod +x "$HOOKS_DEST"/*.sh
  ok "Hooks installed: $HOOKS_DEST"
  warn "Wire hooks in Claude Code: see hooks/claude-code/settings.json.example"
fi

# ── init ──────────────────────────────────────────────────────────────────────
if [[ "$NO_INIT" == false ]]; then
  hdr "Running poneglyph init"
  "$INSTALL_PATH" init
  ok "Config, database, and model cache directories created"
fi

# ── done ──────────────────────────────────────────────────────────────────────
hdr "Done"
ok "poneglyph $("$INSTALL_PATH" --version 2>&1 | head -1)"
echo ""
echo "Next steps:"
echo "  poneglyph serve           # start MCP + HTTP server"
echo "  open http://127.0.0.1:3742  # web viewer"
echo "  poneglyph --help          # all commands"
echo ""
echo "Docs: docs/INTEGRATIONS.md (Claude Code, Claude Desktop, OpenCode)"
