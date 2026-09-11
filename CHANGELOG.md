# Changelog

## 0.3.0 — 2026-09-11

Tagged `v0.3.0`. Operator GO 2026-09-11.

The headline is the desktop graph: `lapis --vault <notes> desktop` opens the
**whole vault**, laid out by a force sim, themed by Omarchy, and steerable with
the mouse and keyboard.

### Graph snapshot

- `Engine::graph_snapshot` returns the whole vault from the embedded index: one
  node per indexed document plus one per dangling link target, one edge per
  wikilink, degree counted in and out.
- Wire shape is `schema/v0.3/graph-snapshot.schema.json`, snake_case like the
  search family. The schema ships in the crate's test data and the test
  validates the emitted JSON against it.
- Above a documented 20 000-node cap the busiest nodes survive and `truncated`
  says so; under it nothing is dropped.
- The hop-ring ego walk is a separate, local thing and is not this API.

### Force layout

- A headless 2D sim: centre, all-pairs repel, link spring, link distance,
  damping, velocity clamp. Ported from the forces Quartz and `obsidian-3d-graph`
  agree on, flattened to two dimensions. No Pixi, d3, Cytoscape or Juggl is a
  dependency, and a test asserts it.
- Repulsion above 64 nodes runs through a Barnes-Hut quadtree. 2 000 nodes and
  4 000 edges tick in about 1.1 ms.
- Settling is damped and it stops: energy falls under a floor, a graph at rest
  stops asking to be ticked, and an interaction wakes it again.
- Seed positions are a golden-angle spiral derived from node count and link
  distance, never hardcoded coordinates.

### Desktop

- Global is the default view, with or without `--path`. `--path` marks the
  active note and seeds local mode; it does not scope the graph.
- Local mode is a breadth-first walk to depth 1..8 over the same snapshot. No
  second index, no second read of the vault.
- Filters: query, existing-only, orphans, and one domain at a time. A filter
  hides exactly what it says it hides.
- Groups colour what a query matches, first match wins, by Omarchy role so a
  theme swap restyles them, with a literal hex as the escape hatch.
- Camera is a transform: drag empty space to pan, wheel or `+`/`-` to zoom about
  the cursor, arrows to pan, `0` to frame the whole graph. Zooming never
  re-runs the forces.
- Hover lights a node, its neighbours and the edges that touch it, and drops
  the rest to a fifth alpha.
- Drag a node to pin it where you drop it; the rest settles around it. Right
  click releases one pin, `u` releases them all, and a short press still opens
  the note.
- Edges are hairlines trimmed to the disc rims, dashed when the link resolves to
  nothing. Disc radius is `2 + sqrt(degree)`.
- Labels are stems, never paths, placed in importance order and dropped rather
  than allowed to overlap. Below a readability zoom none are drawn.
- The void is the Omarchy background and every colour is read live, so a theme
  swap restyles the graph with no restart.
- `LAPIS_GRAPH_DEBUG=1` overlays measured frame rate, worst frame, physics,
  paint and draw cost, node and edge counts, label count and energy.
- On the Linux test host the settle and a drag both hold 60 fps at 2 000 nodes
  and 4 000 edges under nix-ld.

### Keys

`/` query (printable keys are text while it is open, `escape` leaves), `e`
existing-only, `o` orphans, `d` domain, `[` `]` depth, `l` layout, `g` group,
`G` clear groups, `u` unpin, `+` `-` `0` camera, arrows pan.

### Unchanged on purpose

- `neighbors --hop 2` and `tree-retrieve` on the embedded index still say they
  need `lattice.mode = http`. The desktop walking the whole index does not
  license the CLI to grow a quiet hop-2.
- CLI, TUI and `search --embedder none` behave as they did in v0.2.

### Not in this tag

- Windows.
- 3D, edge bundling, hierarchical or circle layouts, minimap.
- Turso / DuckDB as first-run.
- Dummy embeddings.
- Checkpoint tags `v0.2.1`–`v0.2.4`.

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
