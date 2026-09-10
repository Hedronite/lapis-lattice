<p align="center">
  <img src="assets/lapis-poster.jpg" alt="Lapis" width="360">
</p>

<p align="center">
  <strong>Local Markdown vault for humans and agents.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-809DAF?style=flat&colorA=1F2D68" alt="MIT"></a>
  <a href="#status"><img src="https://img.shields.io/badge/Status-beta-809DAF?style=flat&colorA=1F2D68" alt="beta"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/Rust-stable-809DAF?style=flat&colorA=1F2D68&logo=rust&logoColor=809DAF" alt="Rust"></a>
  <a href="https://hedronite.com"><img src="https://img.shields.io/badge/org-hedronite.com-809DAF?style=flat&colorA=1F2D68" alt="org"></a>
</p>

<p align="center">
  Built by <a href="https://github.com/VirtualMachinist">VirtualMachinist</a>.
</p>

---

Lapis is a notes vault for people who work with agents. Notes stay ordinary Markdown files in a folder you own. A TUI, a JSON CLI, and an MCP server share those files. Search and the wikilink graph currently talk to a **Lapis Lattice** HTTP index; an **embedded SQLite + FTS5** index is the 0.2 work so a stranger can install this without a sidecar.

No hosted notes service. Files are the source of truth.

## What it is

| Face | For | Speaks |
|---|---|---|
| **TUI** | you | Vim, preview, tasks, Kanban, dailies, templates, hop-1 graph pane |
| **CLI** | scripts and agents | `lapis --json` |
| **MCP** | coding agents | `lapis mcp` over stdio |
| **Desktop** | preview | `lapis desktop` — off by default (`--features desktop`). A window exists; the visual knowledge-graph canvas is not painted yet |

## Install

Repo: [VirtualMachinist/lapis-lattice](https://github.com/VirtualMachinist/lapis-lattice). Binary: `lapis`. Engine library: [`lapis-lattice`](https://crates.io/crates/lapis-lattice) on crates.io.

**One command** (needs Rust until GitHub Releases exist):

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
```

Or:

```bash
cargo install --git https://github.com/VirtualMachinist/lapis-lattice --locked --bin lapis
lapis init ~/Notes
export LAPIS_VAULT=~/Notes
```

Do **not** run `cargo install lapis` (someone else’s yanked crate), `brew install lapis` (no formula yet), or anything from `lapis.sh` (not us).

Full contract: [docs/install.md](docs/install.md).

## First run

There is **no default vault**. A bare `lapis` without `--vault`, `$LAPIS_VAULT`, or config `vault` exits 1 and tells you to `lapis init`.

Search, neighbors, list, analytics, and tree still need a lattice HTTP server (`http://127.0.0.1:8080` by default). If it is down those commands exit **2**. The TUI still opens the files. Embedded index (no TCP) is 0.2.

```text
lapis init ~/Notes
lapis --vault ~/Notes
lapis read --json Welcome.md
lapis task list --json
```

```json
{ "mcpServers": { "lapis": { "command": "lapis", "args": ["mcp"] } } }
```

## What works / what does not

| Works today | Not yet |
|---|---|
| TUI, JSON CLI envelope, MCP | Search without an HTTP lattice (CLI still HTTP; engine crate is on crates.io) |
| Tasks, dailies, templates (`note`, `daily`, `weekly`, `monthly`, `adr`) | GitHub Release binaries / Homebrew (install.sh builds from git today) |
| Path sandbox, dry-run / mtime / hash guards | Windows |
| Desktop window with `--features desktop` (Metal on macOS) | Visual knowledge-graph **canvas** (data layer only) |
| PDF read-through when the lattice has indexed a tome | YAML/HTML as first-class document types in search |

## Status

**Beta** (0.1): a well-built client over Markdown + an optional HTTP lattice. Not a public one-command app until 0.2 ships the embedded index.

Requirements: macOS or Linux. Embeddings (Ollama / ONNX) are optional. Do **not** install Turso, DuckDB, Xcode, or Python to use the default binary.

Inspired by [ZenNotes](https://github.com/ZenNotes/tui) and Obsidian. Notices: [CREDITS.md](CREDITS.md).
