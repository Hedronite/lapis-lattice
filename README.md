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

Lapis is a notes vault for people who work with agents. Notes stay ordinary Markdown files in a folder you own. A TUI, a JSON CLI, and an MCP server share those files. Search and hop-1 neighbors use an **embedded SQLite + FTS5** index at `<vault>/.lapis/lattice.sqlite` (`meta.producer = lapis-lattice`). An HTTP lattice is opt-in (`lattice.mode = http`).

No hosted notes service. Files are the source of truth.

## What it is

| Face | For | Speaks |
|---|---|---|
| **TUI** | you | Vim, preview, tasks, Kanban, dailies, templates, hop-1 graph pane |
| **CLI** | scripts and agents | `lapis --json` |
| **MCP** | coding agents | `lapis mcp` over stdio |
| **Desktop** | Linux / Omarchy | `lapis desktop` — off in the default CLI asset. The Linux desktop asset paints hop-1/hop-2 (gpui-omarchy). |

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
```

`init` records the vault in `~/.config/lapis/config.toml`, so a bare `lapis`
finds it. `--vault` and `$LAPIS_VAULT` override it per command.

Do **not** run `cargo install lapis` (someone else’s yanked crate), `brew install lapis` (no formula yet), or anything from `lapis.sh` (not us).

Full contract: [docs/install.md](docs/install.md).

## First run

There is **no implicit default vault**: nothing is guessed. A bare `lapis` with no `--vault`, no `$LAPIS_VAULT` and no config `vault` exits 1 and tells you to run `lapis init <path>`, which creates a vault and records it so the next run resolves.

Default search, list, neighbors (hop-1), and doctor talk to the embedded index. They open **no listening socket**. Hop-2 ego and `tree-retrieve` on embedded say they require `lattice.mode = http` rather than failing empty.

```text
lapis init ~/Notes
lapis --vault ~/Notes
lapis --vault ~/Notes --json search welcome
lapis --vault ~/Notes --json list
lapis --vault ~/Notes doctor
```

```json
{ "mcpServers": { "lapis": { "command": "lapis", "args": ["mcp"] } } }
```

## What works / what does not

| Works today | Not yet |
|---|---|
| TUI, JSON CLI envelope, MCP | Windows |
| Embedded search / list / hop-1 / doctor (no `:8080`) | Tagged GitHub Release / Homebrew (install.sh is ready; tag needs GO) |
| Tasks, dailies, templates (`note`, `daily`, `weekly`, `monthly`, `adr`) | Hop-2 ego and `tree-retrieve` on the embedded default (honest HTTP-required) |
| HTML and YAML as first-class kinds | Dummy / fallback embeddings (will not ship) |
| TUI Omarchy live-follow (`colors.toml`) | |
| Linux desktop canvas: hop-1/hop-2, dashed dangling, click-to-open | |

## Status

**Beta** (0.2 on `feat/v0.2-desktop`, not tagged): embedded index is the default. Tag `v0.2.0` only with operator GO.

Requirements: macOS or Linux. Embeddings (Ollama / ONNX) are optional. Do **not** install Turso, DuckDB, Xcode, or Python to use the default binary.

Inspired by [ZenNotes](https://github.com/ZenNotes/tui) and Obsidian. Notices: [CREDITS.md](CREDITS.md).
