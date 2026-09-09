#!/bin/bash
# Castle = control pane. Never write target/ here.
# Usage: scripts/apiary-cargo.sh test
#        scripts/apiary-cargo.sh build --release
set -euo pipefail
REMOTE=apiary
REMOTE_DIR=/Users/abdul-qadir/src/lapis
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"
if [[ "$PWD" == "$REMOTE_DIR" || "$PWD" == "$REMOTE_DIR/"* ]]; then
  exec cargo "$@"
fi
rsync -az --delete --exclude target --exclude .git/objects/pack "$ROOT/" "$REMOTE:$REMOTE_DIR/"
quoted=$(printf '%q ' "$@")
ssh -o BatchMode=yes "$REMOTE" "export PATH=\"\$HOME/.cargo/bin:/opt/homebrew/bin:\$PATH\"; export CARGO_BUILD_JOBS=6; cd $REMOTE_DIR && cargo $quoted"
