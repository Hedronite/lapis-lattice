#!/usr/bin/env bash
# Lapis installer. Spec: docs/install.md
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
#   bash scripts/install.sh --yes --vault "$HOME/Notes"
#
# What this never does: start a daemon, open a port, install Turso, DuckDB,
# Docker, Python, Xcode or Ollama, write outside the bin directory and the vault
# you name, or phone home. Search runs in-process against a SQLite index.
set -euo pipefail

REPO="https://github.com/VirtualMachinist/lapis-lattice"
BIN_DIR="${LAPIS_BIN:-$HOME/.local/bin}"
YES=0
VAULT="${LAPIS_VAULT:-}"
EMBEDDER="none"
DETECT_ONLY=0

usage() {
  cat <<'USAGE'
lapis installer
  --yes                 non-interactive; vault defaults to ~/Notes
  --vault PATH          where notes live
  --embedder none|auto  semantic arm; none is keyword-only and always works
  --bin-dir PATH        install location (default ~/.local/bin)
  --detect              print the detected triple and whether a prebuilt exists, then exit
  -h, --help            this
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --vault) VAULT="${2:-}"; shift ;;
    --embedder) EMBEDDER="${2:-none}"; shift ;;
    --bin-dir) BIN_DIR="${2:-}"; shift ;;
    --detect) DETECT_ONLY=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "lapis-install: unknown flag $1" >&2; usage >&2; exit 1 ;;
  esac
  shift
done

# ---------------------------------------------------------------- triple

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64)   TRIPLE="darwin-arm64" ;;
  Darwin-x86_64)  TRIPLE="darwin-x64" ;;
  Linux-x86_64)   TRIPLE="linux-x64" ;;
  Linux-aarch64|Linux-arm64) TRIPLE="linux-arm64" ;;
  *)
    echo "lapis-install: macOS and Linux only (got $os $arch). Windows is not shipped." >&2
    exit 1
    ;;
esac

# Triples with a published binary. darwin-x64 is deliberately absent: Apple
# Silicon is the Mac we build for, and shipping an untested Intel binary is
# worse than saying so.
has_prebuilt() {
  case "$1" in
    darwin-arm64|linux-x64|linux-arm64) return 0 ;;
    *) return 1 ;;
  esac
}

if [ "$DETECT_ONLY" = 1 ]; then
  if has_prebuilt "$TRIPLE"; then
    echo "$TRIPLE: prebuilt binary available"
  else
    echo "no prebuilt binary for $TRIPLE; install Rust stable and re-run, or build from source."
  fi
  exit 0
fi

mkdir -p "$BIN_DIR"

# ---------------------------------------------------------------- download

installed=0
asset="lapis-${TRIPLE}.tar.gz"

if has_prebuilt "$TRIPLE"; then
  url="$REPO/releases/latest/download/$asset"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  echo "lapis-install: fetching $asset"
  if curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$asset" "$url" 2>/dev/null; then
    # Verify when the release publishes a checksum; a missing one is not fatal,
    # a mismatched one is.
    if curl -fsSL --proto '=https' -o "$tmp/SHA256SUMS" "$REPO/releases/latest/download/SHA256SUMS" 2>/dev/null; then
      want="$(grep " $asset\$" "$tmp/SHA256SUMS" | awk '{print $1}' || true)"
      if [ -n "$want" ]; then
        if command -v shasum >/dev/null 2>&1; then
          got="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
        else
          got="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
        fi
        if [ "$want" != "$got" ]; then
          echo "lapis-install: checksum mismatch for $asset — refusing to install." >&2
          exit 1
        fi
      fi
    fi
    tar -C "$tmp" -xzf "$tmp/$asset"
    install -m 0755 "$tmp/lapis" "$BIN_DIR/lapis"
    installed=1
  else
    echo "lapis-install: no published asset for $TRIPLE yet (no tagged release?)."
  fi
else
  # The exact sentence the spec requires. Never a silent compile.
  echo "no prebuilt binary for $TRIPLE; install Rust stable and re-run, or build from source."
fi

# ------------------------------------------------------- source fallback

if [ "$installed" = 0 ]; then
  if command -v cargo >/dev/null 2>&1; then
    echo "lapis-install: building from source with cargo (this needs a compiler and takes a few minutes)"
    cargo install --git "$REPO" --locked --bin lapis
    src="${CARGO_HOME:-$HOME/.cargo}/bin/lapis"
    [ -x "$src" ] && install -m 0755 "$src" "$BIN_DIR/lapis" && installed=1
  else
    echo "lapis-install: cargo not found. Install Rust stable from https://rustup.rs and re-run." >&2
    exit 1
  fi
fi

# macOS refuses to run a downloaded binary that carries no signature at all.
if [ "$os" = "Darwin" ] && [ -x "$BIN_DIR/lapis" ] && ! "$BIN_DIR/lapis" --version >/dev/null 2>&1; then
  echo "lapis-install: macOS killed the binary (AMFI). Signing it locally:"
  echo "  codesign -s - -f \"$BIN_DIR/lapis\""
  codesign -s - -f "$BIN_DIR/lapis" >/dev/null 2>&1 || true
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo "lapis-install: $BIN_DIR is not on PATH. Add it, then reopen your shell:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

# ---------------------------------------------------------------- vault

if [ -n "$VAULT" ]; then
  :
elif [ "$YES" = 1 ]; then
  VAULT="${HOME}/Notes"
elif [ -t 0 ]; then
  printf "Vault path [%s/Notes]: " "$HOME"
  read -r VAULT || true
  VAULT="${VAULT:-$HOME/Notes}"
else
  echo "lapis-install: installed. Next: lapis init ~/Notes"
  exit 0
fi

# `init` creates the vault, indexes it, and records it in the config, so the
# commands printed below work as typed with no environment to set first.
"$BIN_DIR/lapis" init "$VAULT"
echo "lapis-install: vault $VAULT is configured; override per command with --vault or \$LAPIS_VAULT"
echo
echo "  lapis                     terminal UI"
echo "  lapis search \"welcome\"    keyword search, no daemon"
echo "  lapis doctor              check this install"
echo "  lapis mcp                 speak MCP over stdio"
if [ "$EMBEDDER" != "none" ]; then
  echo
  echo "lapis-install: embedder=$EMBEDDER requested. Keyword search works now;"
  echo "  semantic search turns on once an embedder is configured and the vault is reindexed."
fi
