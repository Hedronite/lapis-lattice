#!/usr/bin/env bash
# Fail if a public clone still leaks operator topology.
# Exception: docs/internal-history.md (not linked from README).
# "Hedronite" is allowed in CREDITS.md and any LICENSE file.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
python3 - <<'PY'
import os, re, sys
pats = [
    r"apiary",
    r"abdul-qadir",
    r"/Users/",
    r"Atrium",
    r"foundry/lapis",
    r"SHIP\.md",
    r"CRATES\.md",
    r"do not cargo",
    r"Castle",
    r"mail-room",
    r"\{\{director\}\}",
    r"Hedronite",
    r"~/Developer",
    r"~/Obsidian",
    r"\blathe\b",
    r"Omahedron",
    r"steam-run",
]
skip_dirs = {".git", "target", "testdata"}
skip_files = {"docs/internal-history.md", "scripts/scrub-gate.sh"}
allow_hedronite = {"CREDITS.md"}
hits = []
for dirpath, dirs, fnames in os.walk("."):
    dirs[:] = [d for d in dirs if d not in skip_dirs and not d.startswith(".")]
    for fn in fnames:
        rel = os.path.normpath(os.path.join(dirpath, fn)).lstrip("./")
        if rel in skip_files:
            continue
        try:
            text = open(os.path.join(dirpath, fn), errors="replace").read()
        except Exception:
            continue
        for pat in pats:
            if pat == r"Hedronite" and (
                rel in allow_hedronite or os.path.basename(rel) == "LICENSE"
            ):
                continue
            for i, line in enumerate(text.splitlines(), 1):
                if re.search(pat, line):
                    hits.append(f"{rel}:{i}: /{pat}/ {line.strip()[:120]}")
if hits:
    print(f"scrub-gate: {len(hits)} hit(s)")
    print("\n".join(hits[:80]))
    if len(hits) > 80:
        print(f"... {len(hits)-80} more")
    sys.exit(1)
print("scrub-gate: clean")
PY
