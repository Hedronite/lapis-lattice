<p align="center">
  <strong>Lapis</strong>
</p>

<p align="center">
  <strong>An agent-first notes and vault app, built for RAG, usable by humans.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-809DAF?style=flat&colorA=1F2D68" alt="MIT"></a>
  <a href="#status"><img src="https://img.shields.io/badge/Status-spec--draft-809DAF?style=flat&colorA=1F2D68" alt="spec-draft"></a>
  <a href="https://go.dev"><img src="https://img.shields.io/badge/Go-1.24-809DAF?style=flat&colorA=1F2D68&logo=go&logoColor=809DAF" alt="Go 1.24"></a>
  <a href="https://hedronite.com"><img src="https://img.shields.io/badge/Hedronite-hedronite.com-809DAF?style=flat&colorA=1F2D68" alt="Hedronite"></a>
</p>

<p align="center">
  <a href="#what-it-is">What it is</a> ·
  <a href="#what-you-get">What you get</a> ·
  <a href="#how-it-fits">How it fits</a> ·
  <a href="#status">Status</a>
</p>

<p align="center">
  Built by <a href="https://hedronite.com">Hedronite</a>’s <a href="https://github.com/VirtualMachinist">VirtualMachinist</a>.
</p>

---

Lapis is a notes vault for people who work with agents. Notes stay ordinary Markdown files in a folder you own. **Lapis Lattice** indexes them for retrieval: lexical search, embeddings, and a wikilink graph. Agents talk to the vault through a JSON CLI and an MCP server. You talk to it through a terminal app — Vim editing, preview, tasks, a Kanban board, daily notes, and a graph view.

The point is one vault both sides can use without a hosted notes service in the middle.

## What it is

A local knowledge base with three faces on the same files:

| Face | For | Speaks |
|---|---|---|
| **TUI** | you | Vim, preview, tasks, Kanban, dailies, templates, graph |
| **CLI** | scripts and agents | `lapis --json` |
| **MCP** | coding agents | `lapis mcp` over stdio |

Underneath, **Lapis Lattice** is the RAG engine: BM25, vector search, fused ranking, and graph neighbors from `[[wikilinks]]`. YAML frontmatter on each note is first-class (title, tags, status, domain) so retrieval can filter as well as search.

## What you get

- **A folder of Markdown** — the vault is files. No proprietary store. Frontmatter is optional on capture and structured when a template creates a note.
- **Lapis Lattice** — hybrid search (lexical + vectors), document metadata, and a link graph. Search is retrieval, not a linear scan of the disk.
- **Agent contract** — every command has `--json`. MCP tools take and return vault-relative `path` values. Tasks have stable IDs (`notes/plan.md#3`) so an agent can toggle a checkbox without rewriting the file by guesswork.
- **Human contract** — a terminal app with splits, a command palette, task lists and boards, daily/weekly notes, and a graph pane of the note you have open.
- **Writes that stay boring** — create, append, rename, trash. The index updates after the file does. Search never mutates the vault.

```text
lapis tui
lapis search --json "hybrid retrieval"
lapis read --json notes/plan.md
lapis task list --json
lapis task toggle notes/plan.md#3
lapis mcp
```

```json
{ "mcpServers": { "lapis": { "command": "lapis", "args": ["mcp"] } } }
```

## How it fits

```text
you ─────────── TUI ──┐
                      │
agent ── CLI / MCP ───┤
                      │
                      ▼
              Markdown on disk
                      │
                      ▼
               Lapis Lattice
          (search, graph, metadata)
```

Use it as a personal wiki, a project vault, or the memory layer next to an agent session. The TUI is for reading and editing. Lattice is for finding. MCP is for giving an agent the same verbs you have, with JSON instead of a screen.

## Status

Spec-draft. The `lapis` binary is not in this tree yet. License is MIT.

Inspired by [ZenNotes](https://github.com/ZenNotes/tui) and Obsidian. Notices: [CREDITS.md](CREDITS.md).
