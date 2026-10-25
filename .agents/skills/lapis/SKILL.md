---
name: lapis
description: >
  Search and read a Lapis notes vault through the lapis CLI. Use when a task
  needs notes, doctrine, project context, wikilink neighbors, or recent notes
  from the vault. Prefer this skill over the lapis MCP server: it adds no tool
  schemas to context and its body is deterministic.
---

# lapis — vault read skill

`lapis` owns the vault and its index. This skill only reads, through the
`--json` CLI contract. Never open `.lapis/` index files directly. The body
is `lapis.nu` beside this file. It needs `nu` and `lapis` on `PATH`.

From this directory:

```bash
nu lapis.nu search "phrase" --limit 5 [--domain d]
nu lapis.nu read path/to/note.md
nu lapis.nu list [--prefix projects/] [--limit 50]
nu lapis.nu neighbors path/to/note.md
nu lapis.nu resolve "[[Note Name]]"
nu lapis.nu recent [--limit 10]
nu lapis.nu status
nu lapis.nu doctor
```

Every command also takes `--vault <path>`.

Each command prints JSON: `{ ok, command, data }` on success, or
`{ ok: false, command, error, exit }` on failure. When `vault info` or
`doctor` printed a report and then exited non-zero, `data` is that report.

- `search`: hybrid retrieval (BM25; vectors only if an embedder is configured), collapsed with `--per-doc` to one hit per note. Returns `path` and `title`. `--limit` defaults to 5; the index caps a page at 50. `--domain` filters the index domain, which is the first folder of the path (`projects/plan.md` is `projects`). A note at the vault root has no domain.
- `read`: frontmatter (`hal`) plus body.
- `list`: index metadata only (`path`, `kind`, `title`, `domain`, `docType`, `status`, `tags`, `updatedAt`, `hash`); never walks the vault. `--prefix` is the `list` positional. The index matches that path prefix and ignores one trailing slash, so `projects` and `projects/` match the same rows. Ordered by path. `--limit` defaults to 50; the index caps a page at 1000. `updatedAt` is milliseconds.
- `neighbors`: hop-1 wikilink edges (the CLI default; hop 2 is not used).
- `resolve`: a `[[wikilink]]` or a neighbors `dst_raw` to a vault path.
- `recent`: `analytics recent`, newest first. The index returns at most 50 rows; `--limit` keeps the first of those (default 10). Cells are chosen by column name, not position. Each result is `{path, title, mtime}`. `mtime` is the `mtime_iso` column when that build has one (0.4.2 rows are `path`, `title`, `domain`, `priority`, `mtime_iso`), otherwise the `mtime` column (older rows are `path`, `title`, `mtime`).
- `status`: `vault info` — vault root, overlay, and index health. Exit 2 when the index is down.
- `doctor`: vault, index, embedder, search, and the path sandbox. Exit 2 when a check fails, including the index being down. A missing embedder is a warning and still exits 0.

## Configuration

The vault path is resolved in this order. There is no implicit default.

1. `--vault <path>` passed to any command
2. `$LAPIS_VAULT`
3. the path recorded by `lapis init` in the lapis config

## Rules

- Read before you cite: give the note path and quote the passage.
- Writes (`create`, `append`, `task`, `trash`) are not part of this skill.
  Run them only when a human has explicitly asked.
- If a path is missing, start `neighbors` from a hub note rather than guessing.
