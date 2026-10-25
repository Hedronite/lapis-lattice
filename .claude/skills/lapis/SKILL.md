---
name: lapis
description: >
  Read, write, and edit a Lapis notes vault through the lapis CLI. Use when a
  task needs notes, search, neighbors, analytics, a tree walk, tasks, a
  guarded in-place edit, or a create, append, or task toggle. Prefer this
  skill over the lapis MCP server: it adds no tool schemas to context and
  its body is deterministic.
---

# lapis

`lapis.nu` beside this file. Needs `nu` and `lapis` on `PATH`. Every command
takes `--vault` (else `$LAPIS_VAULT`, else the path `lapis init` recorded).
Prints `{ ok, command, data }` or `{ ok: false, command, error, stderr, exit }`.
On failure the process exits with that `exit` code. A CLI error's `kind` and
`matches` are copied onto that envelope unchanged. Search rows keep `path`,
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
nu lapis.nu append [--dry-run] [--body text] -- PATH TEXT
nu lapis.nu replace [--dry-run] [--no-reindex] --old TEXT --new TEXT [--if-hash HASH] [--if-mtime MS] -- PATH
nu lapis.nu insert [--dry-run] [--no-reindex] (--after ANCHOR | --before ANCHOR) --text TEXT [--if-hash HASH] [--if-mtime MS] -- PATH
nu lapis.nu task list [--path p] [--full] [--limit n] [--status s] [--due when] [--tag t]
nu lapis.nu task toggle [--dry-run] -- ID
nu lapis.nu tome build [--allow-windows] [--force] -- PDF...
nu lapis.nu tome tree DOC [--node id] [--depth n]
nu lapis.nu tome open DOC NODE...
```

Flags go before `--`. `nu lapis.nu append --dry-run -- PATH "- [ ] x"`. A value that starts with `--` uses `--flag=value` or `--stdin`. Words after PATH are one text.
`replace` and `insert` are the agent edit path. Read the note and pass `data.hash` as `--if-hash`. `--stdin` stands in for `--new` or `--text`. The match is the raw body bytes. Zero or many matches leave the file unchanged.
`tome build`, `tome tree`, and `tome open` need a `lapis` built with `--features tome`.
Run `create`, `append`, and `task toggle` only when a human has asked.
