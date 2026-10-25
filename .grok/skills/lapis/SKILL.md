---
name: lapis
description: >
  Read and write a Lapis notes vault through the lapis CLI. Use when a task
  needs notes, search, neighbors, analytics, a tree walk, tasks, or a create,
  append, or task toggle. Prefer this skill over the lapis MCP server: it
  adds no tool schemas to context and its body is deterministic.
---

# lapis

`lapis.nu` beside this file. Needs `nu` and `lapis` on `PATH`. Every command
takes `--vault` (else `$LAPIS_VAULT`, else the path `lapis init` recorded).
Prints `{ ok, command, data }` or `{ ok: false, command, error, stderr, exit }`.
On failure the process exits with that `exit` code. Search rows keep `path`,
`title`, `score`, and `snippet`.

```bash
nu lapis.nu search "phrase" --limit 5 [--domain d]
nu lapis.nu read path/to/note.md
nu lapis.nu list [--prefix projects/] [--limit 50]
nu lapis.nu neighbors path/to/note.md
nu lapis.nu resolve "[[Note Name]]"
nu lapis.nu recent [--limit 10]
nu lapis.nu analytics QUERY
nu lapis.nu tree-retrieve [--path p] [--query text] [--depth n] [--max-nodes n]
nu lapis.nu status
nu lapis.nu doctor
nu lapis.nu create --title TITLE [--path p] [--type t] [--domain d] [--tag [a b]] [--body text] [--dry-run]
nu lapis.nu append PATH TEXT [--dry-run] [--if-mtime ms] [--if-hash hash]
nu lapis.nu task list [--path p] [--full] [--limit n] [--status s] [--due when] [--tag t]
nu lapis.nu task toggle ID [--dry-run] [--if-mtime ms] [--if-hash hash]
nu lapis.nu tome tree DOC [--node id] [--depth n]
nu lapis.nu tome open DOC NODE...
```

`tome tree` and `tome open` need a `lapis` built with `--features tome`.
Run `create`, `append`, and `task toggle` only when a human has explicitly asked.
