# Changelog

## Unreleased — v0.2.0

Not tagged. Do not treat this as a GitHub Release until the operator says GO.

### Embedded default

- `lapis init` then `lapis search` / `lapis list` / hop-1 `neighbors` / `doctor` use `<vault>/.lapis/lattice.sqlite`.
- No listening socket on `:8080`. HTTP lattice is opt-in (`lattice.mode = http`).
- Search flags `--offset`, `--domain`, `--per-doc`, `--mode` reach the engine.
- List rows include nullable `hash` (camelCase `docType` / `updatedAt`).
- Writes reindex that path only (`reindex_path`).
- `meta.producer = lapis-lattice` on sqlite open.
- Hop-2 and `tree-retrieve` on embedded say they require `lattice.mode = http` (not a silent empty).

### Optional vectors

- `--embedder none` omits the vector arm. Embedder failure never writes dummy embeddings.
- `auto` is resolved once and persisted; search does not probe per query.

### Kinds

- HTML and YAML are first-class indexer kinds.

### Theme

- TUI Omarchy live-follow reads exactly `colors.toml`, ignores `hyprland_*`, applies the collision rule, watches parent `current/`.
- Fixture: `testdata/omarchy/current/`. Brand fallback when `current/` is missing.

### Install

- Default CLI assets are desktop-off (`darwin-arm64`, `linux-x64`, `linux-arm64`).
- `darwin-x64` is an honest refuse.
- Second asset: `lapis-desktop-linux-*`.
- `scripts/install.sh` downloads release assets. Never `cargo install lapis`, `brew install lapis`, or `lapis.sh`.

### Desktop (Linux / Omarchy)

- Window is **gpui-omarchy** + gpui-kit. Zed-gpui `mod window` is gone.
- Hop-1/hop-2 canvas paints: discs, labels, straight edges, dashed dangling, click opens the note, domain and dangling filters.
- Linux binary is compiled on GitHub Actions `ubuntu-latest`. Smoke on lathe via `steam-run`. A missing `libxcb` / `libwayland` / `libvulkan` `.so` **without** steam-run is host config, not a product fail.

### Not in this tag

- Windows.
- Turso / DuckDB as first-run.
- Dummy embeddings.
- Merging `main` or tagging without operator GO.
