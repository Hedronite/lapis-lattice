# Install spec — one command / one click

Product: **Lapis** (CLI/TUI/MCP binary `lapis`).
Repo: [VirtualMachinist/lapis-lattice](https://github.com/VirtualMachinist/lapis-lattice).
Library crate: [`lapis-lattice`](https://crates.io/crates/lapis-lattice) on crates.io (embedded SQLite+FTS5 engine).

This is the contract for a stranger’s first minute. The one-command path
downloads a GitHub Release (`v0.3.1` as of 2026-09-11) on Apple Silicon and
Linux. Homebrew, Developer ID notarization, and Intel Mac prebuilts are still
open. Names that look convenient and are **wrong** are listed first so we do
not bake them in.

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
2. A vault exists and is named: `--vault`, `$LAPIS_VAULT`, or the `vault` key `lapis init` writes
3. `lapis --version` prints
4. `lapis doctor` is green (once `doctor` exists)

`install.sh` **downloads the matching Release asset** and does not require a
compiler. If there is no asset for this arch (`darwin-x64` today), it falls
back to `cargo` when Rust is on `PATH`, otherwise it tells you to install
Rust (`https://rustup.rs`) and exits 1.

## One click (target)

| Click | What it does |
|---|---|
| GitHub Release asset (`lapis-<os>-<arch>.tar.gz`) | Download + extract + `install` into `~/.local/bin` |
| Homebrew | Not published. Do not `brew install lapis`. Target later: `brew install virtualmachinist/tap/lapis` |
| macOS `.pkg` (later) | Drag-less installer; needs Developer ID + notarization. Today's Release is ad-hoc / linker-signed only |
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
10. Do **not** start `localhost:8080`. Default lattice is **embedded**. CLI
    `search` / `list` / hop-1 / `doctor` talk to `<vault>/.lapis/lattice.sqlite`.
11. On macOS, Release assets are ad-hoc / linker-signed, not notarized. A
    browser download carries `com.apple.quarantine` and Gatekeeper/`spctl`
    will reject it. `install.sh` (curl) clears quarantine when it can, then
    if `--version` still dies: `xattr -d com.apple.quarantine` and
    `codesign -s - -f "$BIN_DIR/lapis"`, and say so. Manual workaround:

    ```bash
    xattr -d com.apple.quarantine "$BIN_DIR/lapis"
    codesign -s - -f "$BIN_DIR/lapis"
    ```

## `lapis setup` / `lapis doctor` (needed for the command to be true)

`lapis init <path>` is the whole of it: it creates the vault, indexes it, and
writes `vault` into `~/.config/lapis/config.toml` so the next bare `lapis`
resolves without an environment variable. `--vault` and `$LAPIS_VAULT` still win
over the config. An `init` of somewhere else **never retargets** a vault the
config already names — it prints `left alone` and tells you to pass `--vault`
or `$LAPIS_VAULT`. There is no `lapis config set vault`. There is no separate
`setup` command. `doctor` reports:

- binary version, OS/arch
- vault path and whether it is a directory
- lattice: `embedded` (sqlite at `<vault>/.lapis/lattice.sqlite`) or `http` URL, reachable?
- embedder: `none` / `ollama` / `onnx`
- PATH

Exit codes stay: 0 ok, 1 usage, 2 lattice/index down, 3 path.

## Wiring (what is true on `v0.3.1`)

Shipped:

- GitHub repo is `lapis-lattice`. Tagged Releases attach `lapis-darwin-arm64`, `lapis-linux-x64`, `lapis-linux-arm64`, plus `SHA256SUMS`.
- `cargo add lapis-lattice` gets the engine library (crates.io `0.1.0`).
- CLI binary `lapis` defaults to the **embedded** index. HTTP is opt-in (`lattice.mode = "http"` or `--lattice`).
- `lapis init` writes `<vault>/.lapis/lattice.sqlite` and records the vault in the config (first time only).
- `lapis doctor` exists.
- `scripts/install.sh` downloads the matching asset.
- `pack-release` CI: macOS + Linux, desktop **off** on the CLI asset.

Still open (not a reason to claim search needs `:8080`):

- Homebrew tap / formula.
- Developer ID + notarization (browser-downloaded macOS assets fail Gatekeeper).
- Intel Mac prebuilt.
- TUI palette search still uses the HTTP client, so a first-run TUI shows `lattice ✗` and Ctrl+P dies on `:8080` even when CLI search is green. File browsing still works.
- `lapis --version` prints crate `0.1.0`, not the Git tag.

## Click-path copy for a future landing page

Primary:

```bash
curl -fsSL https://raw.githubusercontent.com/VirtualMachinist/lapis-lattice/main/scripts/install.sh | bash
```

With Rust already:

```bash
cargo install --git https://github.com/VirtualMachinist/lapis-lattice --locked --bin lapis
lapis init ~/Notes
```

Engine only (library, not the app):

```bash
cargo add lapis-lattice
```
