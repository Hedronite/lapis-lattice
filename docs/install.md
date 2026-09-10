# Install spec — one command / one click

Product: **Lapis** (CLI/TUI/MCP binary `lapis`).
Repo: [VirtualMachinist/lapis-lattice](https://github.com/VirtualMachinist/lapis-lattice).
Library crate: [`lapis-lattice`](https://crates.io/crates/lapis-lattice) on crates.io (embedded SQLite+FTS5 engine).

This is the contract for a stranger’s first minute. It is **not** fully shipped yet. Names that look convenient and are **wrong** are listed first so we do not bake them in.

## Names we do not own

| Looks like | Reality | Do not ship |
|---|---|---|
| `cargo install lapis` | crates.io `lapis` is a yanked 0.0.0 (“Lapis lazuli”), not us | Yes: never document this |
| `brew install lapis` | No formula on homebrew/core | Not until we publish a tap/formula |
| `https://lapis.sh/install.sh` | `lapis.sh` 301s to an unrelated Carrd | Yes: never use that host |
| `cargo install lapis-lattice` | Installs the **library** crate, no `lapis` binary | Do not present as the app installer |

## One command (target)

Host the script **in this repo** so we do not need a custom domain:

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
```

Non-interactive:

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash -s -- \
  --yes --vault "$HOME/Notes" --embedder none
```

That must exit 0 only when:

1. `lapis` is on `PATH` (default `$HOME/.local/bin`)
2. A vault exists (`--vault` or `$LAPIS_VAULT` or `lapis init`)
3. `lapis --version` prints
4. `lapis doctor` is green (once `doctor` exists)

Until GitHub Releases exist, `install.sh` may build from this git repo with `cargo` (Rust stable). After Releases exist, it **downloads the matching binary** and does not require a compiler.

## One click (target)

| Click | What it does |
|---|---|
| GitHub Release asset (`lapis-<os>-<arch>.tar.gz`) | Download + extract + `install` into `~/.local/bin` |
| Homebrew | `brew install virtualmachinist/tap/lapis` (tap we publish; formula name `lapis` is the **binary**) |
| macOS `.pkg` (later) | Drag-less installer; adhoc-signed until we have a Developer ID |
| README / docs.rs “install” button | Must copy one of the commands above, never `cargo install lapis` |

Windows: installer prints “macOS and Linux only” and exits 1 until we have a job.

## What `install.sh` does (exact sequence)

1. Detect OS/arch: `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64`. Refuse anything else.
2. Resolve `BIN_DIR="${LAPIS_BIN:-$HOME/.local/bin}"`. `mkdir -p`.
3. If a GitHub Release matching `v*` has an asset for this arch, download, verify SHA256, install `lapis` into `BIN_DIR`.
4. Else if `cargo` is on `PATH`, run:
   `cargo install --git https://github.com/VirtualMachinist/lapis-lattice --locked --bin lapis --root "$BIN_DIR/.."`  
   (or `CARGO_INSTALL_ROOT` so the binary lands in `BIN_DIR`).
5. Else print: install Rust (`https://rustup.rs`) **or** download a Release asset, and exit 1.
6. If `BIN_DIR` is not on `PATH`, print the one-liner to add it (`export PATH="$HOME/.local/bin:$PATH"`).
7. If `--yes` or non-TTY: `lapis init "${LAPIS_VAULT:-$HOME/Notes}"` when the vault is missing.
8. If TTY and no `--yes`: ask vault path, create if missing, optional MCP snippet.
9. Do **not** SSH, rsync, install Turso, Docker, Python, DuckDB, Xcode, or Ollama. Embeddings are optional (`--embedder none|auto|ollama|onnx`).
10. Do **not** start `localhost:8080`. Default lattice is **embedded** once the CLI is wired to `lapis-lattice`; until then, `lapis doctor` must say “search needs embedded index (0.2) or HTTP lattice” instead of pretending.
11. On macOS, if Gatekeeper SIGKILLs a downloaded binary: `codesign -s - -f "$BIN_DIR/lapis"` and say so.

## `lapis setup` / `lapis doctor` (needed for the command to be true)

`lapis init` already creates a vault. `setup` is init + config + first index. `doctor` reports:

- binary version, OS/arch
- vault path and whether it is a directory
- lattice: `embedded` (sqlite at `<vault>/.lapis/lattice.sqlite`) or `http` URL, reachable?
- embedder: `none` / `ollama` / `onnx`
- PATH

Exit codes stay: 0 ok, 1 usage, 2 lattice/index down, 3 path.

## Wiring (why this is still a spec)

Today:

- GitHub repo is `lapis-lattice`.
- `cargo add lapis-lattice` gets the engine library.
- The CLI package in this repo is still named `lapis` (binary `lapis`). It still talks HTTP lattice by default.
- `lapis init` does **not** yet write `<vault>/.lapis/lattice.sqlite` via the library.

0.2 makes the one-command true:

1. CLI `Backend { Embedded(lapis_lattice::Engine), Http(...) }` with **embedded default**.
2. `lapis init` / `setup` indexes into `.lapis/lattice.sqlite`.
3. `lapis doctor`.
4. `scripts/install.sh` as above.
5. `pack-release` CI: macOS + Linux, desktop **off**, checksums, GitHub Release.
6. Homebrew tap.

Until (1)–(3) land, `install.sh` may install the binary and init a vault, but `lapis search` can still exit 2 without an HTTP lattice. The script must say that in its closing lines. Do not claim `curl | bash` is done while search still needs `:8080`.

## Click-path copy for a future landing page

Primary:

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
```

With Rust already:

```bash
cargo install --git https://github.com/VirtualMachinist/lapis-lattice --locked --bin lapis
lapis init ~/Notes
export LAPIS_VAULT=~/Notes
```

Engine only (library, not the app):

```bash
cargo add lapis-lattice
```
