#!/usr/bin/env bash
set -euo pipefail

# ── colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'; BOLD='\033[1m'; RESET='\033[0m'
ok()   { echo -e "${GREEN}✓${RESET} $*"; }
warn() { echo -e "${YELLOW}!${RESET} $*"; }
die()  { echo -e "${RED}✗${RESET} $*" >&2; exit 1; }
hdr()  { echo -e "\n${BOLD}$*${RESET}"; }

# ── defaults ──────────────────────────────────────────────────────────────────
PURGE=false
YES=false

# ── args ──────────────────────────────────────────────────────────────────────
for arg in "$@"; do
  case "$arg" in
    --purge) PURGE=true ;;
    --yes)   YES=true ;;
    --help|-h)
      echo "Usage: $0 [--purge] [--yes]"
      echo ""
      echo "  --purge   Remove config, data, and model cache (irreversible)"
      echo "  --yes     Non-interactive, keep data directories (safe default)"
      exit 0
      ;;
    *) die "Unknown flag: $arg. Run with --help for usage." ;;
  esac
done

# ── find binary ───────────────────────────────────────────────────────────────
hdr "Locating poneglyph"

INSTALL_PATH=""
if [[ -f "$HOME/.local/bin/poneglyph" ]]; then
  INSTALL_PATH="$HOME/.local/bin/poneglyph"
elif [[ -f "/usr/local/bin/poneglyph" ]]; then
  INSTALL_PATH="/usr/local/bin/poneglyph"
else
  die "poneglyph not found in ~/.local/bin or /usr/local/bin"
fi
ok "Found: $INSTALL_PATH"

# ── stop server ───────────────────────────────────────────────────────────────
hdr "Stopping server"

if pkill -f "poneglyph mcp" 2>/dev/null; then
  ok "Server stopped"
else
  ok "No running server found"
fi

# ── remove binary ─────────────────────────────────────────────────────────────
hdr "Removing binary"

if [[ "$INSTALL_PATH" == /usr/local/bin/* ]]; then
  sudo rm "$INSTALL_PATH"
else
  rm "$INSTALL_PATH"
fi
ok "Removed: $INSTALL_PATH"

# ── data directories ──────────────────────────────────────────────────────────
# Data lives under CONFIG_DIR (.config/poneglyph/data) as of v1.1.0; the
# .local/share path is kept here too in case it's a pre-1.1.0 install that
# never moved.
CONFIG_DIR="$HOME/.config/poneglyph"
LEGACY_DATA_DIR="$HOME/.local/share/poneglyph"
CACHE_DIR="$HOME/.cache/poneglyph"

if [[ "$PURGE" == false && "$YES" == false ]]; then
  hdr "Data directories"
  echo "  Config + data:  $CONFIG_DIR"
  echo "  Legacy data:    $LEGACY_DATA_DIR (if present)"
  echo "  Cache:          $CACHE_DIR"
  echo ""
  warn "These contain your memories, config, and downloaded model (~30MB)."
  read -r -p "Remove all data? [y/N] " reply
  [[ "$reply" =~ ^[Yy]$ ]] && PURGE=true
fi

if [[ "$PURGE" == true ]]; then
  hdr "Removing data"
  warn "This is irreversible — all stored memories will be lost."

  if [[ "$YES" == false ]]; then
    read -r -p "Confirm purge? [y/N] " confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { echo "Purge cancelled."; exit 0; }
  fi

  [[ -d "$CONFIG_DIR"      ]] && rm -rf "$CONFIG_DIR"      && ok "Removed: $CONFIG_DIR"
  [[ -d "$LEGACY_DATA_DIR" ]] && rm -rf "$LEGACY_DATA_DIR" && ok "Removed: $LEGACY_DATA_DIR"
  [[ -d "$CACHE_DIR"       ]] && rm -rf "$CACHE_DIR"       && ok "Removed: $CACHE_DIR"
fi

# ── done ──────────────────────────────────────────────────────────────────────
hdr "Done"
ok "poneglyph uninstalled"

if [[ "$PURGE" == false ]]; then
  echo ""
  warn "Data directories kept:"
  echo "  Config:  $CONFIG_DIR"
  echo "  Data:    $DATA_DIR"
  echo "  Cache:   $CACHE_DIR"
  echo ""
  echo "To remove them manually: rm -rf $CONFIG_DIR $DATA_DIR $CACHE_DIR"
fi
