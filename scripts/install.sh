#!/usr/bin/env bash
# Lapis installer. Spec: docs/install.md
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
#   bash scripts/install.sh --yes --vault "$HOME/Notes"
set -euo pipefail

REPO="https://github.com/VirtualMachinist/lapis-lattice"
BIN_DIR="${LAPIS_BIN:-$HOME/.local/bin}"
YES=0
VAULT="${LAPIS_VAULT:-}"
EMBEDDER="none"

usage() {
  sed -n '2,6p' "$0"
  echo "flags: --yes  --vault PATH  --embedder none|auto|ollama|onnx  --bin-dir PATH"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --yes|-y) YES=1 ;;
    --vault) VAULT="${2:-}"; shift ;;
    --embedder) EMBEDDER="${2:-none}"; shift ;;
    --bin-dir) BIN_DIR="${2:-}"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "lapis-install: unknown flag $1" >&2; usage; exit 1 ;;
  esac
  shift
done

os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64|Darwin-x86_64|Linux-x86_64|Linux-aarch64) ;;
  *)
    echo "lapis-install: macOS and Linux only (got $os $arch). Windows is not shipped." >&2
    exit 1
    ;;
esac

mkdir -p "$BIN_DIR"

if command -v cargo >/dev/null 2>&1; then
  echo "lapis-install: building lapis from $REPO (needs Rust; Release binaries come later)"
  cargo install --git "$REPO" --locked --bin lapis
  src="${CARGO_HOME:-$HOME/.cargo}/bin/lapis"
  if [ -x "$src" ]; then
    install -m 0755 "$src" "$BIN_DIR/lapis"
  fi
else
  echo "lapis-install: cargo not found. Install Rust from https://rustup.rs" >&2
  echo "  or wait for a GitHub Release asset and re-run." >&2
  exit 1
fi

if ! command -v lapis >/dev/null 2>&1; then
  case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
      echo "lapis-install: add to PATH, then retry:"
      echo "  export PATH=\"$BIN_DIR:\$PATH\""
      ;;
  esac
fi

if [ -n "$VAULT" ]; then
  :
elif [ "$YES" = 1 ]; then
  VAULT="${HOME}/Notes"
elif [ -t 0 ]; then
  printf "Vault path [%s/Notes]: " "$HOME"
  read -r VAULT || true
  VAULT="${VAULT:-$HOME/Notes}"
else
  echo "lapis-install: binary installed. Set LAPIS_VAULT or run: lapis init ~/Notes"
  echo "lapis-install: search still needs an HTTP lattice or the 0.2 embedded backend. embedder=$EMBEDDER"
  exit 0
fi

if command -v lapis >/dev/null 2>&1; then
  lapis init "$VAULT" || true
  echo "lapis-install: export LAPIS_VAULT=$VAULT"
  echo "lapis-install: search still needs HTTP lattice (:8080) or the 0.2 embedded backend. Not claiming doctor-green yet."
else
  echo "lapis-install: lapis built into $BIN_DIR — open a new shell if it is not on PATH."
fi
