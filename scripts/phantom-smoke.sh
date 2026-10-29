#!/bin/sh
# Live Phantom smoke (G5f): build the pinned phantom-fractal and run Lapis
# against it. Run from anywhere inside the lapis-lattice clone; read-only
# against Phantom, which is cloned and built outside this repo.
#
#   LAPIS         absolute path to a lapis built from the PR head on this host
#   PHANTOM_REPO  Phantom's public clone URL
#
# Every step prints its exit code; a step that should fail does not stop the
# list. Fetch and build stop it, since nothing after them can run.
set -u
: "${LAPIS:?set LAPIS to the absolute path of a lapis built from the PR head}"
: "${PHANTOM_REPO:?set PHANTOM_REPO to the Phantom clone URL}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PIN="$(cat "$ROOT/PHANTOM_PIN")"   # 486e5f3fa359a0abf1029c4df413aa581896cd7f
SRC="$HOME/src/phantom-pin/$PIN"
OUT="$HOME/.local/share/lapis/phantom-fractal/$PIN"
echo "phantom pin: $PIN"
echo "lapis head: $(git -C "$ROOT" rev-parse HEAD)"

# 1. Fetch Phantom at the pin (public repo, read-only clone)
[ -d "$SRC/.git" ] || git clone --filter=blob:none "$PHANTOM_REPO" "$SRC" || exit 1
git -C "$SRC" checkout --detach "$PIN" || exit 1
test "$(git -C "$SRC" rev-parse HEAD)" = "$PIN" || exit 1

# 2. Build only the phantom-fractal binary
cd "$SRC" || exit 1
cargo build --release --locked -p phantom-fractal --bin phantom-fractal || exit 1
mkdir -p "$OUT"
install -m 0755 target/release/phantom-fractal "$OUT/phantom-fractal" || exit 1
"$OUT/phantom-fractal" --json version; echo "exit=$?"   # envelope, exit 0, "schema":1; git_sha == $PIN or null (unknown, not a failure)

# 3. Phantom alone: real envelopes. Record each exit code and schema
"$OUT/phantom-fractal" index "$SRC"; echo "exit=$?"     # writes a Phantom store in the host's data dir; the evidence notes where
"$OUT/phantom-fractal" find Envelope --repo "$SRC"; echo "exit=$?"
"$OUT/phantom-fractal" callers parse --repo "$SRC"; echo "exit=$?"
"$OUT/phantom-fractal" find nothing_matches_zzz --repo "$SRC"; echo "exit=$?"
# each stdout must be one JSON envelope with "schema":1

# 4. Lapis against the pinned binary (--json is global, so either position works)
LAPIS_PHANTOM_BINARY="$OUT/phantom-fractal" "$LAPIS" desktop --check; echo "exit=$?"         # prints "phantom version: ok", exit 0
LAPIS_PHANTOM_BINARY="$OUT/phantom-fractal" "$LAPIS" --json desktop --check; echo "exit=$?"  # phantom.ok true; record phantom.data, git_sha included

# 5. Negative cases
env -u LAPIS_PHANTOM_BINARY PATH=/usr/bin:/bin "$LAPIS" desktop --check; echo "exit=$?"     # "Phantom not found", exit 0
LAPIS_PHANTOM_BINARY=phantom-fractal "$LAPIS" desktop --check; echo "exit=$?"               # E-1: warning names the variable only
# wrong-schema stub: a script named phantom-fractal that prints {"ok":true,"data":null,"error":null,"schema":2} for any arguments
STUB_DIR="$(mktemp -d)"
printf '%s\n' '#!/bin/sh' "printf '%s' '{\"ok\":true,\"data\":null,\"error\":null,\"schema\":2}'" > "$STUB_DIR/phantom-fractal"
chmod 0755 "$STUB_DIR/phantom-fractal"
LAPIS_PHANTOM_BINARY="$STUB_DIR/phantom-fractal" "$LAPIS" desktop --check; echo "exit=$?"    # schema refusal, non-zero
rm -rf "$STUB_DIR"
