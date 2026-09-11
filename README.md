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

**One command** — `install.sh` downloads the latest GitHub Release for Apple Silicon, Linux x64, or Linux arm64. Rust is only needed if there is no asset for your machine (Intel Mac today):

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
```

Or, from source:

```bash
cargo install --git https://github.com/VirtualMachinist/lapis-lattice --locked --bin lapis
lapis init ~/Notes
```

`init` records the vault in `~/.config/lapis/config.toml`, so a bare `lapis`
finds it. `--vault` and `$LAPIS_VAULT` override it per command. A second
`init` of a different path creates that vault but does **not** retarget the
config; use `--vault` / `$LAPIS_VAULT`, or edit the `vault` key.

Do **not** run `cargo install lapis` (someone else’s yanked crate), `brew install lapis` (no formula or tap yet), or anything from `lapis.sh` (not us).

**macOS (Apple Silicon).** The Release binary is ad-hoc / linker-signed, not Developer ID + notarized. `curl | bash` is the supported path. A browser download of `lapis-darwin-arm64.tar.gz` is quarantined; Gatekeeper/`spctl` will reject it until you clear the flag and (if needed) ad-hoc sign:

```bash
xattr -d com.apple.quarantine ./lapis
codesign -s - -f ./lapis
```

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
| TUI (files / editor / tasks), JSON CLI envelope, MCP | Windows |
| Embedded CLI search / list / hop-1 / doctor (no `:8080`) | Homebrew tap / formula; macOS Developer ID + notarization |
| Tasks, dailies, templates (`note`, `daily`, `weekly`, `monthly`, `adr`) | Hop-2 ego and `tree-retrieve` on the embedded default (honest HTTP-required) |
| HTML and YAML as first-class kinds | Dummy / fallback embeddings (will not ship) |
| TUI Omarchy live-follow (`colors.toml`) | TUI palette search on the embedded default (still talks HTTP `:8080`) |
| Linux desktop canvas: hop-1/hop-2, dashed dangling, click-to-open | Intel Mac prebuilt (`darwin-x64` is an honest refuse) |

## Status

**Beta** (`v0.3.1` tagged). Embedded index is the default. `lapis --version` still prints the crate version (`0.1.0`); believe the Git tag, not that string.

Requirements: macOS (Apple Silicon prebuilt) or Linux. Embeddings (Ollama / ONNX) are optional. Do **not** install Turso, DuckDB, Xcode, or Python to use the default binary.

Inspired by [ZenNotes](https://github.com/ZenNotes/tui) and Obsidian. Notices: [CREDITS.md](CREDITS.md).
