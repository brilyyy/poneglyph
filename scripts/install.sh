#!/usr/bin/env bash
set -euo pipefail

GH_REPO="brilyyy/poneglyph"

# ── colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'; BOLD='\033[1m'; RESET='\033[0m'
ok()   { echo -e "${GREEN}✓${RESET} $*"; }
warn() { echo -e "${YELLOW}!${RESET} $*"; }
die()  { echo -e "${RED}✗${RESET} $*" >&2; exit 1; }
hdr()  { echo -e "\n${BOLD}$*${RESET}"; }

# ── defaults ──────────────────────────────────────────────────────────────────
SKIP_BUILD=false
SYSTEM=false
NO_INIT=false
YES=false

# ── args ──────────────────────────────────────────────────────────────────────
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=true ;;
    --system)     SYSTEM=true ;;
    --no-init)    NO_INIT=true ;;
    --yes)        YES=true ;;
    --help|-h)
      echo "Usage: $0 [--skip-build] [--system] [--no-init] [--yes]"
      echo ""
      echo "  --skip-build   Use existing target/release/poneglyph (skip build)"
      echo "  --system       Install to /usr/local/bin (requires sudo)"
      echo "  --no-init      Skip running 'poneglyph init' after install"
      echo "  --yes          Non-interactive, accept all defaults"
      exit 0
      ;;
    *) die "Unknown flag: $arg. Run with --help for usage." ;;
  esac
done

# ── temp dir cleanup ─────────────────────────────────────────────────────────
CLONE_DIR=""
cleanup() { [[ -n "$CLONE_DIR" ]] && rm -rf "$CLONE_DIR"; return 0; }
trap cleanup EXIT

# ── are we already inside a clone? ──────────────────────────────────────────
# Empty when piped via `curl | bash` ($0 has no meaningful directory); set
# when run as ./scripts/install.sh from a real checkout.
SOURCE_DIR=""
CANDIDATE="$(cd "$(dirname "$0")/.." 2>/dev/null && pwd || true)"
if [[ -n "$CANDIDATE" && -f "$CANDIDATE/Cargo.toml" && -f "$CANDIDATE/scripts/build-release.sh" ]]; then
  SOURCE_DIR="$CANDIDATE"
fi

# ── prebuilt binary fast path ───────────────────────────────────────────────
# Mirrors scripts/npm-postinstall.js's target-triple mapping. No release has
# been cut yet as of this writing, so this is expected to fail today and fall
# through to the clone+build path below — but it's the preferred path once
# releases start shipping, and costs nothing to try first.
target_triple() {
  local os arch
  os="$(uname -s)"; arch="$(uname -m)"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64)  echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) return 1 ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        *) return 1 ;;
      esac
      ;;
    *) return 1 ;;
  esac
}

PREBUILT_BINARY=""
try_install_prebuilt() {
  local target asset url tmp_gz tmp_bin
  command -v curl &>/dev/null || return 1
  target="$(target_triple)" || return 1
  asset="poneglyph-${target}.gz"
  url="https://github.com/${GH_REPO}/releases/latest/download/${asset}"
  tmp_gz="$(mktemp)"
  if ! curl -fsSL "$url" -o "$tmp_gz" 2>/dev/null; then
    rm -f "$tmp_gz"
    return 1
  fi
  tmp_bin="$(mktemp)"
  if ! gzip -dc "$tmp_gz" > "$tmp_bin" 2>/dev/null; then
    rm -f "$tmp_gz" "$tmp_bin"
    return 1
  fi
  rm -f "$tmp_gz"
  chmod +x "$tmp_bin"
  PREBUILT_BINARY="$tmp_bin"
  return 0
}

clone_repo() {
  command -v git &>/dev/null || die "git not found. Install git, or build from source — see README.md."
  CLONE_DIR="$(mktemp -d)"
  hdr "Cloning poneglyph"
  git clone --depth 1 "https://github.com/${GH_REPO}.git" "$CLONE_DIR" 2>&1 | sed 's/^/  /'
  ok "Cloned to $CLONE_DIR"
}

BINARY=""

if [[ "$SKIP_BUILD" == false ]]; then
  hdr "Checking for a prebuilt binary"
  if try_install_prebuilt; then
    ok "Downloaded prebuilt binary for $(target_triple)"
    BINARY="$PREBUILT_BINARY"
  else
    warn "No prebuilt binary available — building from source"
  fi
fi

# ── build from source (only if the fast path above didn't produce a binary) ─
if [[ -z "$BINARY" ]]; then
  if [[ "$SKIP_BUILD" == true && -z "$SOURCE_DIR" ]]; then
    die "--skip-build requires running from inside a cloned repo (no local checkout found)."
  fi

  if [[ -z "$SOURCE_DIR" ]]; then
    clone_repo
    SOURCE_DIR="$CLONE_DIR"
  fi
  cd "$SOURCE_DIR"

  # ── prereqs ─────────────────────────────────────────────────────────────
  hdr "Checking prerequisites"

  if ! command -v rustc &>/dev/null; then
    die "Rust not found. Install via https://rustup.rs"
  fi
  RUST_VERSION=$(rustc --version | awk '{print $2}')
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

  # ── build ───────────────────────────────────────────────────────────────
  if [[ "$SKIP_BUILD" == false ]]; then
    BUILD_FEATURES="viewer"
    if [[ "$YES" == false ]]; then
      hdr "Choose an LLM provider to compile in"
      echo "Powers background enrichment/compression. Each provider adds its own"
      echo "HTTP client dependency, so only what you pick gets compiled in."
      echo ""
      echo "  1) OpenAI-compatible — also covers Ollama, LM Studio, gpt4all (base_url)"
      echo "  2) Anthropic"
      echo "  3) Gemini"
      echo ""
      echo -n "Selection (space-separated, Enter for none): "
      read -r llm_selection
      for num in $llm_selection; do
        case "$num" in
          1) BUILD_FEATURES="$BUILD_FEATURES,llm-openai" ;;
          2) BUILD_FEATURES="$BUILD_FEATURES,llm-anthropic" ;;
          3) BUILD_FEATURES="$BUILD_FEATURES,llm-gemini" ;;
          *) warn "Unknown option: $num (skipping)" ;;
        esac
      done
    fi
    hdr "Building poneglyph (features: $BUILD_FEATURES)"
    bash scripts/build-release.sh "$BUILD_FEATURES"
  else
    hdr "Skipping build (--skip-build)"
  fi

  BINARY="target/release/poneglyph"
  [[ -f "$BINARY" ]] || die "Binary not found at $BINARY. Run without --skip-build."
fi

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

# ── init ──────────────────────────────────────────────────────────────────────
if [[ "$NO_INIT" == false ]]; then
  hdr "Running poneglyph init --config"
  "$INSTALL_PATH" init --config
  ok "Global config, database, and model cache directories created"
  echo "Run \`poneglyph init\` inside each project you want it wired into."
fi

# ── feature selection ────────────────────────────────────────────────────────
prompt_features() {
  hdr "Choose features to set up"
  echo "Select which features to enable (space-separated numbers, Enter for all):"
  echo ""
  echo "  1) Claude Code hooks    — auto-capture prompts, responses, tool usage"
  echo "  2) OpenCode plugin      — same capture for OpenCode editor"
  echo "  3) MCP server           — memory/codegraph tools for any MCP client"
  echo "  4) Web dashboard        — browser UI for browsing memories & graph"
  echo ""
  echo -n "Selection [1 2 3 4]: "
  read -r selection

  # Default: all features
  if [[ -z "$selection" ]]; then
    selection="1 2 3 4"
  fi

  FEATURES_CLAUDE=false
  FEATURES_OPENCODE=false
  FEATURES_MCP=false
  FEATURES_VIEWER=false

  for num in $selection; do
    case "$num" in
      1) FEATURES_CLAUDE=true ;;
      2) FEATURES_OPENCODE=true ;;
      3) FEATURES_MCP=true ;;
      4) FEATURES_VIEWER=true ;;
      *) warn "Unknown option: $num (skipping)" ;;
    esac
  done
}

if [[ "$YES" == true ]]; then
  FEATURES_CLAUDE=true
  FEATURES_OPENCODE=true
  FEATURES_MCP=true
  FEATURES_VIEWER=true
else
  prompt_features
fi

# ── wire features ───────────────────────────────────────────────────────────
if [[ "$FEATURES_CLAUDE" == true ]]; then
  hdr "Wiring Claude Code hooks"
  "$INSTALL_PATH" wire claude-code 2>/dev/null && ok "Claude Code hooks installed" || warn "Could not wire Claude Code (may need manual setup)"
fi

if [[ "$FEATURES_OPENCODE" == true ]]; then
  hdr "Wiring OpenCode plugin"
  "$INSTALL_PATH" wire opencode 2>/dev/null && ok "OpenCode plugin installed" || warn "Could not wire OpenCode (may need manual setup)"
fi

if [[ "$FEATURES_MCP" == true ]]; then
  echo ""
  echo "To start the MCP server:"
  echo "  poneglyph mcp"
  echo ""
  echo "Add to your MCP client config:"
  echo '  "poneglyph": { "command": "poneglyph", "args": ["mcp"] }'
fi

if [[ "$FEATURES_VIEWER" == true ]]; then
  echo ""
  echo "To start the web dashboard:"
  echo "  poneglyph viewer"
fi

# ── done ──────────────────────────────────────────────────────────────────────
hdr "Done"
ok "poneglyph $("$INSTALL_PATH" --version 2>&1 | head -1)"
echo ""
echo "Docs: docs/INTEGRATIONS.md (Claude Code, Claude Desktop, OpenCode)"
