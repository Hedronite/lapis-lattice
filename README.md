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

> **Status:** In production use as daily-driver / dogfood Markdown vault (TUI/CLI/MCP + embedded FTS5). Hardening: beta. Latest GitHub Release and crates.io `lapis-lattice` are `0.4.1`. `0.4.2` (this tree, including `--rerank-jev`) is the next tag. Not a toy reference.


Lapis is a notes vault for people who work with agents. Notes stay ordinary Markdown files in a folder you own. A TUI, a JSON CLI, a skill, and an MCP server share those files. Search and hop-1 neighbors use an **embedded SQLite + FTS5** index at `<vault>/.lapis/lattice.sqlite` (`meta.producer = lapis-lattice`). An HTTP lattice is opt-in (`lattice.mode = http`).

No hosted notes service. Files are the source of truth.

## What it is

| Face | For | Speaks |
|---|---|---|
| **TUI** | you | Vim, preview, tasks, Kanban, dailies, templates, hop-1 graph pane |
| **CLI** | scripts and agents | `lapis --json` |
| **Skill** | coding agents (default) | `.agents/skills/lapis/` over `lapis --json` |
| **MCP** | harnesses that defer tool schemas, or have no shell | `lapis mcp` over stdio |
| **Desktop** | Linux / Omarchy | `lapis desktop` — off in the default CLI asset. The Linux desktop asset paints hop-1/hop-2 (gpui-omarchy). |

## Agent use: skills + CLI first, MCP still shipped

The default way for an agent to use Lapis is the bundled skill in
`.agents/skills/lapis/`: a short `SKILL.md` the harness reads to decide when to use
it, plus a Nushell body (`lapis.nu`) that calls `lapis --json`. The body needs
`nu` and `lapis` on `PATH`. The same bytes are regular files at
`.claude/skills/lapis/`, `.grok/skills/lapis/`, and `.cursor/skills/lapis/`.
`scripts/sync-skills` rewrites those copies from `.agents/skills/`. The MCP
server (`lapis mcp`) is still built and shipped, and works the same as before.

Why the skill is the default:

- **Context.** An MCP server adds every tool's name, description and input
  schema to the model's context, often on every request, whether or not the
  tools are used. Many harnesses can only turn a whole server on or off. A
  skill costs a few dozen tokens of description until it is used.
- **Determinism.** The skill body is fixed code over a versioned JSON
  contract, so results are the same each time.
- **Small models.** Local models with short context windows feel the schema
  cost most.

Use the MCP server when your harness defers or filters MCP tool schemas
(loading them only on demand, or letting you enable single tools), or when
the client has no shell.

A checkout already has the project copies. To install the skill for every
project on one machine, copy `.agents/skills/lapis/` into a user skills
directory (for example `~/.agents/skills/` or `~/.claude/skills/`).

## Install

Repo: [Hedronite/lapis-lattice](https://github.com/Hedronite/lapis-lattice).

| Artifact | Install | What you get |
|---|---|---|
| **CLI / TUI / MCP** (`lapis` binary) | `install.sh` or `cargo install --git … --bin lapis` | The app: vault, TUI, JSON CLI, MCP |
| **Engine library** (`lapis-lattice`) | `cargo add lapis-lattice` (live: `0.4.1`) | Embedded SQLite+FTS5 index crate — no `lapis` binary |

**One command** — `install.sh` downloads the latest GitHub Release binary (currently `v0.4.1`, `lapis --version` → `0.4.1`) for Apple Silicon, Linux x64, or Linux arm64. Rust is only needed if there is no asset for your machine (Intel Mac today):

```bash
curl -fsSL https://raw.githubusercontent.com/Hedronite/lapis-lattice/main/scripts/install.sh | bash
```

Or, from source (app binary):

```bash
cargo install --git https://github.com/Hedronite/lapis-lattice --locked --bin lapis
lapis init ~/Notes
```

Engine only (library, not the app). Live on crates.io today: `0.4.1`. After
`v0.4.2` is tagged and published:

```bash
cargo add lapis-lattice@0.4.2
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

Search is keyword (BM25) until you name an embedder. With [Ollama](https://ollama.com) running, add this to `~/.config/lapis/config.toml` and run `lapis reindex --all`; `search` is then hybrid (BM25 + vectors):

```toml
[embedder]
provider = "ollama"            # or "auto", or "none"
model = "nomic-embed-text"
ollama_url = "http://127.0.0.1:11434"
timeout_ms = 5000              # per query; index_timeout_ms (120000) per batch
```

If Ollama is down or slow, search falls back to BM25 and the stored vectors stay; `lapis reindex --vectors` later embeds whatever was missed. `search --embedder none` skips the vector arm for one query. `lapis doctor` reports the embedder, how many chunks have vectors, and one query round trip.

```text
lapis init ~/Notes
lapis --vault ~/Notes
lapis --vault ~/Notes --json search welcome
lapis --vault ~/Notes --json search welcome --rerank-jev
lapis --vault ~/Notes --json list
lapis --vault ~/Notes doctor
```

## In-place edit

`lapis` 0.4.2 could only add text. `replace` and `insert` change a note in place. The match is the raw bytes of the body (everything after the closing frontmatter `---`, or the whole file when there is no frontmatter). There is no regex and no CRLF or trailing-newline folding. Overlapping hits count, so `aa` in `aaa` is two matches. Zero or many matches exit 1, leave the file byte-identical, and the JSON error carries `error.matches`.

The recommended agent path is read, then edit under that hash:

```text
lapis --vault ~/Notes --json read notes/plan.md
lapis --vault ~/Notes --json replace notes/plan.md --old 'exact text' --new 'replacement' --if-hash fnv1a64:…
lapis --vault ~/Notes --json insert notes/plan.md --after 'anchor' --text 'inserted' --if-hash fnv1a64:…
lapis --vault ~/Notes replace notes/plan.md --old 'exact text' --new 'replacement' --dry-run
```

`--if-hash` and `--if-mtime` are the same stale guards as `append`. The hash is FNV-1a 64 over the exact on-disk bytes (`hashAlgo` is `fnv1a64` in the JSON; `oldHash` / `newHash` use that prefix). A mismatch exits 1 and writes nothing. `--dry-run` prints a unified diff, writes nothing, does not bump `updated:` on disk, and does not reindex. Its `newHash` is the unbumped preview, so it excludes the `updated:` line a real write stores. The dry-run still runs the same frontmatter and wikilink gate as that write, on the bumped text, and fails when the write would refuse. `--stdin` reads the replacement or the inserted text.

A real write is a temp file in the note's directory, then rename, and it keeps the original file mode. It bumps top-level `updated:` to today, or inserts that line immediately before the closing `---` when the key is absent, and leaves every other frontmatter byte unchanged. A note with no frontmatter block stays headerless. The frontmatter block is not searched, so a needle that lives only there is zero matches. The index kick reindexes that one path. `..`, an absolute path, and a symlink that resolves outside the vault exit 3 and touch nothing.

`--json` errors from these commands set `error.kind` and `error.exit`. `kind` is `not_found` or `sandbox` (both exit 3), or `stale_guard`, `match_count`, or `bad_args` (exit 1). `match_count` also sets `error.matches`.

MCP exposes one tool, `lapis_edit`. `mode` is an enum: `replace`, `insert_after`, or `insert_before`. Same guards and the same match rule. A tool error's `data` carries `kind` and `exit`, plus `matches` when the kind is `match_count`. The base server is 15 tools.

Coding agents with a shell should use the skill in `.agents/skills/lapis/` (see Agent use, above). This stdio config is for harnesses that talk MCP:

```json
{ "mcpServers": { "lapis": { "command": "lapis", "args": ["mcp"] } } }
```

## What works / what does not

| Works today | Not yet |
|---|---|
| TUI (files / editor / tasks), JSON CLI envelope, skill (`.agents/skills/lapis/`), MCP | Windows |
| Embedded CLI search / list / hop-1 / doctor (no `:8080`) | Homebrew tap / formula; macOS Developer ID + notarization |
| `lapis search … --rerank-jev` (shadow Jev gate; Facet TypeSafe) | TUI palette Jev rerank |
| Tasks, dailies, templates (`note`, `daily`, `weekly`, `monthly`, `adr`) | Hop-2 ego and `tree-retrieve` on the embedded default (honest HTTP-required) |
| HTML and YAML as first-class kinds | Dummy / fallback embeddings (will not ship) |
| TUI Omarchy live-follow (`colors.toml`) | TUI palette search on the embedded default (still talks HTTP `:8080`) |
| Linux desktop canvas: hop-1/hop-2, dashed dangling, click-to-open | Intel Mac prebuilt (`darwin-x64` is an honest refuse) |

## Status

**Beta**.

| | Live today | Next (after `v0.4.2` tag + `cargo publish`) |
|---|---|---|
| GitHub Release binary | `v0.4.1` (`lapis --version` → `0.4.1`) | `v0.4.2` binary asset (`--version` → `0.4.2`) |
| crates.io `lapis-lattice` | [`0.4.1`](https://crates.io/crates/lapis-lattice/0.4.1) | `0.4.2` (`cargo add lapis-lattice@0.4.2`) |
| `main` (this repo) | — | Cargo `0.4.2`; `lapis --version` → `0.4.2` |

The `v0.4.0` Release binary still reports crate `0.1.0` and is not rewritten. `cargo add lapis-lattice` resolves to **0.4.1** until `0.4.2` is published. Neither that library pin nor `cargo install lapis` is the app binary.

Requirements: macOS (Apple Silicon prebuilt) or Linux. Embeddings (Ollama / ONNX) are optional. Do **not** install Turso, DuckDB, Xcode, or Python to use the default binary.

Inspired by [ZenNotes](https://github.com/ZenNotes/tui) and Obsidian. Notices: [CREDITS.md](CREDITS.md).
