#!/usr/bin/env python3
"""Create a deterministic, synthetic UX benchmark vault. Never overwrites files.

The 10,000 notes total approximately 100 MiB. The first 1,000 form a
3,000-edge connected graph; the remainder include sparse links and orphans.
Run each benchmark with its own copy, recording index and embedder state.
"""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--out", required=True)
args = parser.parse_args()
root = Path(args.out).resolve()
if root.exists() and any(root.iterdir()):
    raise SystemExit("Output must be empty; use a new disposable fixture directory.")
root.mkdir(parents=True, exist_ok=True)


def path(number):
    return f"Collection-{number // 100:03d}/Note-{number:05d}.md"


records = []
for number in range(10_000):
    links = [(number + step) % 1000 for step in (1, 7, 17)] if number < 1000 else (
        [number - 1] if number % 7 else []
    )
    content = (
        f"---\ntitle: Note {number:05d}\ndomain: synthetic\n"
        f"tags: [ux-fixture, collection-{number // 100:03d}]\n"
        f"custom: preserve-{number:05d}\n---\n\n# Note {number:05d}\n\n"
        "A synthetic knowledge workspace note for repeatable UI measurements.\n\n"
        "## Connections\n\n" + "\n".join(f"[[{path(n)[:-3]}]]" for n in links) + "\n\n"
        "## Working notes\n\n- [ ] Review the reference material\n- [x] Capture the source\n\n"
        "| Field | Value |\n| --- | --- |\n| Purpose | Repeatable measurement |\n\n"
        "```yaml\nworkspace: synthetic\nindentation:\n  preserved: true\n```\n\n"
    )
    paragraph = "Reading, retrieval, editing, and navigation use the same permitted corpus. "
    while len(content.encode()) + len(paragraph) + 2 <= 10_486:
        content += paragraph + "\n\n"
    content += " " * (10_486 - len(content.encode()) - 1) + "\n"
    data = content.encode()
    note = root / path(number)
    note.parent.mkdir(exist_ok=True)
    note.write_bytes(data)
    records.append({"path": path(number), "bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})

manifest = {
    "id": "lapis-ux-standard-v1",
    "privacy": "synthetic",
    "notes": len(records),
    "bytes": sum(record["bytes"] for record in records),
    "dense_subset": {"first_note": 0, "nodes": 1000, "edges": 3000, "offsets": [1, 7, 17]},
    "index_state": "absent; build through Lapis before indexed measurements",
    "files": records,
}
encoded = (json.dumps(manifest, sort_keys=True, indent=2) + "\n").encode()
(root / "fixture-manifest.json").write_bytes(encoded)
print(json.dumps({"notes": manifest["notes"], "bytes": manifest["bytes"], "manifest_sha256": hashlib.sha256(encoded).hexdigest()}))
