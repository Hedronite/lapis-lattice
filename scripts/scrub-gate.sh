#!/usr/bin/env bash
# Fail if a public clone still leaks operator topology.
# Scans tracked files only (`git ls-files`), dot-directories included.
# Exception: docs/internal-history.md (not linked from README).
# "Hedronite" is allowed in CREDITS.md, any LICENSE file, and the canonical
# public repo URL (github.com/Hedronite/lapis-lattice). Other mentions stay banned.
# Every other exception is an exact literal scoped to one file (ALLOW below).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
git ls-files -z | python3 -c "$(cat <<'PY'
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
    r"\bcastle\b",
    r"\bcitadel\b",
    r"\btower\b",
    r"mail-room",
    r"mail_room",
    r"\{\{director\}\}",
    r"Hedronite",
    r"~/Developer",
    r"~/Obsidian",
    r"\blathe\b",
    r"Omahedron",
    r"steam-run",
    r"archmagus",
    r"09-Tomes",
    r"evanpincham",
    r"\bEvan\b",
]
pats = [(p, re.compile(p, re.IGNORECASE)) for p in pats]
skip_files = {"docs/internal-history.md", "scripts/scrub-gate.sh"}
allow_hedronite = {"CREDITS.md"}
# Longer forms first so a full URL and a markdown label both redact cleanly.
canonical_slugs = (
    "https://raw.githubusercontent.com/Hedronite/lapis-lattice",
    "https://github.com/Hedronite/lapis-lattice",
    "Hedronite/lapis-lattice",
)
# Exact literals allowed in one file. A literal is removed from the line before
# the patterns run again, so anything else on the same line (or a new line in the
# same file) still fails.
# Shipped vault defaults (RAG gate scope, task scan excludes, taxonomy folder):
# renaming them changes product behavior, so they stay until a separate change
# makes them configurable.
MAIL = "agents/mail_room/"
ALLOW = {
    "src/cli.rs": [MAIL],
    "src/write.rs": [MAIL],
    "src/rag_gate.rs": [MAIL, "agents/mail_room.md", "agents/mail_room/**"],
    "src/tasks.rs": ["\"projects/atrium-lattice/data/\"", "\"Archmagus-Stack/09-Tomes/\""],
    # Public org site badge and the gate-result schema \$id (an identifier, not a host).
    "README.md": ["https://hedronite.com", "org-hedronite.com"],
    "schema/gate-result.schema.json": ["https://hedronite.local/foundry/rag-gate/gate-result.schema.json"],
    "src/taxonomy.rs": ["`projects/atrium-lattice/taxonomy.py`", "\"projects/atrium-lattice\""],
}
# The `tower` crate (an upstream Rust dependency) in lockfiles only.
LOCK_ALLOW = ["name = \"tower\"", "\"tower\",", "\"tower ", "\"tower-"]
ALLOW = {k: sorted(v, key=len, reverse=True) for k, v in ALLOW.items()}

files = [f for f in sys.stdin.buffer.read().decode().split("\0") if f]
hits = []
for rel in files:
    rel = os.path.relpath(os.path.normpath(rel))
    if rel in skip_files or "testdata" in rel.split(os.sep) or not os.path.isfile(rel):
        continue
    try:
        text = open(rel, errors="replace").read()
    except Exception:
        continue
    allow = ALLOW.get(rel, [])
    if os.path.basename(rel) == "Cargo.lock":
        allow = allow + LOCK_ALLOW
    for i, line in enumerate(text.splitlines(), 1):
        redacted = line
        for lit in allow:
            redacted = redacted.replace(lit, "")
        for pat, rx in pats:
            probe = redacted
            if pat == r"Hedronite":
                if rel in allow_hedronite or os.path.basename(rel) == "LICENSE":
                    continue
                for slug in canonical_slugs:
                    probe = probe.replace(slug, "")
            if rx.search(probe):
                hits.append(f"{rel}:{i}: /{pat}/i {line.strip()[:120]}")
if hits:
    print(f"scrub-gate: {len(hits)} hit(s)")
    print("\n".join(hits[:80]))
    if len(hits) > 80:
        print(f"... {len(hits)-80} more")
    sys.exit(1)
print("scrub-gate: clean")
PY
)"
