<p align="center">
  <strong>Lapis</strong>
</p>

<p align="center">
  <strong>Operator TUI, JSON CLI, and MCP for an Atrium vault.</strong><br>
  Lattice recalls. Markdown and HAL store. Tasks, Kanban, dailies, and a graph on top.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-809DAF?style=flat&colorA=1F2D68" alt="MIT"></a>
  <a href="#status"><img src="https://img.shields.io/badge/Status-spec--draft-809DAF?style=flat&colorA=1F2D68" alt="spec-draft"></a>
  <a href="https://go.dev"><img src="https://img.shields.io/badge/Go-1.24-809DAF?style=flat&colorA=1F2D68&logo=go&logoColor=809DAF" alt="Go 1.24"></a>
  <a href="https://hedronite.com"><img src="https://img.shields.io/badge/Hedronite-hedronite.com-809DAF?style=flat&colorA=1F2D68" alt="Hedronite"></a>
</p>

<p align="center">
  <a href="#what-it-is">What it is</a> ·
  <a href="#what-it-is-not">What it is not</a> ·
  <a href="#intended-surface">Surface</a> ·
  <a href="#lattice">Lattice</a> ·
  <a href="#status">Status</a> ·
  <a href="#credits">Credits</a>
</p>

<p align="center">
  Built by <a href="https://hedronite.com">Hedronite</a>’s <a href="https://github.com/VirtualMachinist">VirtualMachinist</a>.<br>
  <em>Product mark lands in <code>assets/</code> — lapis blue <code>#1F2D68</code>, regent grey <code>#809DAF</code>.</em>
</p>

---

Lapis is the operator and agent face of an [Atrium](https://hedronite.com) vault: one Go binary that speaks `--json`, MCP stdio, and a Vim TUI, bound to **Atrium Lattice** (hybrid search, wikilink graph, HAL frontmatter) instead of a four-bucket notes app.

Obsidian stays a second IDE on the same files. Lattice stays the recall engine. Lapis does not write `lattice.db`.

**spec-draft** · binary not shipped · vault spec in Atrium `foundry/lapis/`

## What it is

| | Lapis |
|---|---|
| Store | Plain Markdown + HAL YAML on disk (the same vault Obsidian opens) |
| Recall | Atrium Lattice — BM25 + vectors + `edges`, via `127.0.0.1:8080` |
| Agents | `lapis --json` and `lapis mcp` (trust the `path` other tools return) |
| Operator | TUI: Vim, preview, tasks, Kanban, daily notes, templates, graph pane |
| Tasks | Checkbox grammar with stable IDs (`path#n`) so agents toggle work without editing YAML |

The top layer (CLI, MCP, TUI, tasks, Kanban, dailies, templates) is the shape we wanted from [ZenNotes `zn`](https://github.com/ZenNotes/tui). The memory layer is ours. This is not a fork of `zn`: we would have thrown most of it away. We copy chrome under MIT and credit it. See [CREDITS.md](CREDITS.md).

## What it is not

- Not `zn`. Different binary, different MCP name, different vault contract.
- Not a rewrite of the Atrium folder tree into inbox / quick / archive / trash. Those are overlays on dirs that already exist.
- Not the lathe Halo roles vault (`~/Obsidian/Lapis` on lathe). Same word, different object.
- Not Obsidian Sync, Canvas, or mobile.
- Not a second writer of `lattice.db`. Indexer and reconcile own the rows.

## Intended surface

```text
lapis tui                 # operator app
lapis search --json Q     # hybrid lattice search
lapis read --json PATH
lapis task list --json
lapis task toggle PATH#0
lapis mcp                 # stdio MCP for Halo / Claude Code / Codex
```

```json
{ "mcpServers": { "lapis": { "command": "lapis", "args": ["mcp"] } } }
```

Writes go to Markdown. HAL is stamped on create (templates), then agents prefer `toggle_task` / `append` over patching frontmatter. Lapis may rewrite only an allowlist of HAL keys (`status`, `priority`, `tags`, `updated`, `name`).

## Lattice

Lapis is a **client** of Atrium Lattice:

- Walk rule: `is_walked` (do not reimplement)
- Search: `GET /search`
- Graph: `edges` / neighbors (hop-2 is a lattice route, not a second wikilink parser)
- Health: `GET /healthz`

If lattice is down, search fails with a clear exit code. The TUI must still open files.

## Status

Spec-draft. No `lapis` binary in this tree yet. Law, schemas, and the milestone checklist live in the Atrium vault under `foundry/lapis/` (SPEC, CHECKLIST, STATUS).

Milestones, short:

| | Gate |
|---|---|
| M1 | Read CLI against live lattice (`search`, `read`, `vault info`, `neighbors`) |
| M2 | MCP + HAL create/append + index kick |
| M3 | TUI shell (sidebar, Vim/preview, search palette, HAL inspector) |
| M4 | Tasks, Kanban, dailies, templates, graph pane |
| M5 | Tree retrieve + health in the palette |

## Repository map

```
README.md          # this file
LICENSE            # MIT
CREDITS.md         # ZenNotes and other notices
assets/            # product mark (forthcoming)
cmd/lapis/         # entrypoint (forthcoming)
```

## Contributing

Issues and pull requests are welcome once M1 exists. Until then, the contract is the vault spec.

```bash
go test ./...
```

## Credits

Lapis is built and maintained by [Hedronite](https://hedronite.com).

TUI/Vim/task-grammar chrome is adapted from [ZenNotes/tui](https://github.com/ZenNotes/tui) (MIT, Copyright 2026 Adib Hanna and ZenNotes contributors). Full notice: [CREDITS.md](CREDITS.md).
