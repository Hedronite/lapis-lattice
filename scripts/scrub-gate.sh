#!/usr/bin/env bash
# Fail if a public clone still leaks operator topology.
# Scans tracked files only (`git ls-files`), dot-directories included.
# Exception: docs/internal-history.md (not linked from README).
# "Hedronite" is allowed in CREDITS.md, any LICENSE file, and the canonical
# public repo URL (github.com/Hedronite/lapis-lattice). Other mentions stay banned.
# Every other exception is an exact literal scoped to one file (ALLOW below).
# testdata/ directories are scanned like everything else (no directory skip).
# Fails closed: no file list, an empty list, or an unreadable tracked file is
# a failure, never "clean".
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "scrub-gate: not inside a git work tree" >&2
    exit 1
fi
LIST="$(mktemp)"
trap 'rm -f "$LIST"' EXIT
if ! git ls-files -z >"$LIST"; then
    echo "scrub-gate: git ls-files failed" >&2
    exit 1
fi
python3 - "$LIST" <<'PY'
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
    # Tailscale CGNAT range 100.64.0.0/10 (100.64.x.x through 100.127.x.x).
    r"(?<![\d.])100\.(?:6[4-9]|[7-9]\d|1[01]\d|12[0-7])\.\d{1,3}\.\d{1,3}(?![\d.]*\d)",
    r"ts\.net\b",
    r"tailnet",
    r"protonmail",
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
ALLOW = {
    "src/cli.rs": ["`agents/mail_room/**`)"],
    "src/write.rs": ["`agents/mail_room/**`)"],
    "src/rag_gate.rs": [
        "//! and `agents/mail_room/**`.",
        "or `agents/mail_room/`. Other trees",
        "rel.starts_with(\"agents/mail_room/\")",
        "and agents/mail_room/** markdown create/touch\"",
        "in_scope(\"agents/mail_room/notes/2026-09-21.md\")",
        "in_scope(\"agents/mail_room.md\")",
        "\"agents/mail_room/m.md\",",
    ],
    "src/tasks.rs": ["\"projects/atrium-lattice/data/\"", "\"Archmagus-Stack/09-Tomes/\""],
    # Public org site badge and the gate-result schema $id (an identifier, not a host).
    "README.md": ["href=\"https://hedronite.com\"", "/badge/org-hedronite.com-809DAF?"],
    "crates/lapis-lattice/testdata/graph-snapshot.schema.json": [
        "\"https://hedronite.local/lapis/schema/v0.3/graph-snapshot.schema.json\""
    ],
    "schema/gate-result.schema.json": ["https://hedronite.local/foundry/rag-gate/gate-result.schema.json"],
    "src/taxonomy.rs": ["`projects/atrium-lattice/taxonomy.py`", "\"projects/atrium-lattice\""],
}
# The `tower` crate (an upstream Rust dependency) in the tracked workspace
# lockfiles only (repo root and the vendored workspace root).
LOCK_FILES = {"Cargo.lock", "vendor/gpui-base/Cargo.lock"}
LOCK_ALLOW = ["name = \"tower\"", "\"tower\",", "\"tower ", "\"tower-"]
ALLOW = {k: sorted(v, key=len, reverse=True) for k, v in ALLOW.items()}

files = [f for f in open(sys.argv[1], "rb").read().decode().split("\0") if f]
if not files:
    print("scrub-gate: git ls-files returned no files", file=sys.stderr)
    sys.exit(1)
hits = []
unreadable = []
for rel in files:
    rel = os.path.relpath(os.path.normpath(rel))
    if rel in skip_files:
        continue
    if os.path.islink(rel) or os.path.isdir(rel):
        continue  # a symlink or submodule entry: no file content of its own
    try:
        with open(rel, errors="replace") as fh:
            text = fh.read()
    except Exception as e:
        unreadable.append(f"{rel}: {e.__class__.__name__}: {e}")
        continue
    allow = ALLOW.get(rel, [])
    if rel in LOCK_FILES:
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
if unreadable:
    print(f"scrub-gate: {len(unreadable)} unreadable tracked file(s)")
    print("\n".join(unreadable))
if hits:
    print(f"scrub-gate: {len(hits)} hit(s)")
    print("\n".join(hits[:80]))
    if len(hits) > 80:
        print(f"... {len(hits)-80} more")
if hits or unreadable:
    sys.exit(1)
print(f"scrub-gate: clean ({len(files)} tracked files)")
PY
