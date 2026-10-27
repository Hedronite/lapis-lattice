# Changelog

## [Unreleased]

### Graph frame budget and logging

- A dashed graph link is added only when its dashes fit the frame's segment budget as well as the dash budget, so a frame never passes the budget the renderer is sized for.
- The desktop's stderr logger writes the home folder as `~`, so a pasted log does not name the machine's user.

### Vault containment

- New files stay inside the vault. `create` (and `capture`, `daily` and the other periodic notes), `restore`, `trash` and the desktop's Save a copy used to join the path to the vault root and write, so a folder symlinked out of the vault could carry the write outside it. They now go through `notes::contained_new`. It resolves the deepest folder that already exists, refuses one outside the vault (exit 3), and refuses a target that exists in any form, a dangling symlink included. The write then lands under that folder's real path, without following the links again.
- The index walk no longer follows links. A symlinked folder is never walked: its notes are indexed where they really live, so it cannot lead out of the vault or loop back into itself. A symlinked file is indexed only when it resolves inside the vault, and a single-path reindex checks the same.
- Reads open the canonical file (`notes::resolve_target`), so a link swapped after the containment check cannot redirect the read. Edits still go through the lexical path, so the shared writer keeps refusing to replace a symlink.

### Graph: fast frames, local drags

- Small notes draw as batched hexagons, one shape per colour, instead of one disc each. On a 13,000-note vault, per-disc overhead was most of every frame. The board's draw time at rest fell from about 23 ms to about 5 ms, and a fresh layout unfolds at about 55 fps where it ran at 7–10. Hubs, pinned, focused and unresolved notes still draw as real circles, on top.
- A drag moves its neighbourhood and nothing else. Dragging or releasing a note frees that note and the notes up to two links from it (at most 1,500) until the layout rests. The rest of the map stays still, so a drag no longer lets the whole graph keep reorganising toward its true resting shape. Physics ticks during a drag fell to about 1 ms.
- The speed cap cools with the layout: a fresh layout opens out at full speed, and a drag or a warm start moves at most a fifth of it.

### Graph clusters

- A hub keeps its notes around it, and connected notes form visible clusters. On a 13,000-note vault the median distance from a busy note to its own notes fell from more than half the graph's radius to about a sixth. Among the notes nearest a hub, the share that are its own rose from about 1% to about 40%.
- The cause was the start, not the forces. Notes started on a golden-angle spiral as wide as the finished graph, in path order, so linked notes began on opposite sides of a big vault and could never travel far enough to meet before the layout cooled. They now start in link order (breadth-first from the busiest note) along an evenly spaced spiral no wider than six link lengths, and the speed cap rose from 40 to 200 so a compact start can open out. A small graph still starts at its natural size, and a chain starts unfolded.
- Repulsion runs across all cores: 13,000 notes settle in about 1.3 s. Below a twentieth of a link length, rather than half, two notes count as that close, so close neighbours keep pushing apart. The separation gap rose from 0.25 to 0.35 of a link length. A layout is now the same every run: the separation pass used to visit a hash map in a different order each time.
- Each linked pair is now one spring. A→B and B→A, or a link written twice, used to count twice.
- Remembered maps from the previous layout are not reopened, because the memory file version changed. The next open lays the vault out afresh.

### Graph memory

- The whole-vault graph remembers its shape. When the layout comes to rest, after it opens or once a dragged note settles, every note's position and every pin is saved per vault in `$XDG_CACHE_HOME/lapis/graph/` (else `~/.cache/lapis/graph/`). The cache never goes in the vault.
- The next open starts from that memory when the two mostly agree: at least half the graph is remembered and at least half the memory is still in the graph, so a vault that lost half its notes gets a fresh map. Known notes start where they were, a new note starts beside its remembered neighbours, and the layout begins cool, so the map opens in its familiar shape and only the seams settle. Pins dropped in an earlier session come back.
- Saving a note no longer re-runs the whole layout. The reload starts warm from the map on screen, so the graph adjusts in place.
- A missing, damaged or foreign-version memory file only means a fresh layout. Local and ring views keep their own layouts and are not remembered.

### What the index covers

- The embedded index skips nested vaults: a folder below the vault root that holds its own `.lapis/` or `.obsidian/` is left to its own index. Test fixtures that build sample vaults inside a real vault no longer show up in search and the graph.
- `.lapisignore` at the vault root takes gitignore patterns (anchors, `**`, negation) for generated output inside the vault. A full reindex, the stale count and a single-path reindex all follow it. Saving a file inside an ignored folder removes it from the index instead of indexing it.
- One vault had 12,733 generated test notes inside it, which filled its graph past the 20,000-node snapshot cap and its search with fixtures.

### Vectors in the embedded index

- `[embedder]` in the config turns on the vector arm of the embedded index. `provider = "ollama"` with `model` and `ollama_url` (defaults `nomic-embed-text`, `http://127.0.0.1:11434`); nomic models get their `search_query:` / `search_document:` prefixes. `timeout_ms` (5000) bounds a query, `index_timeout_ms` (120000) a batch of chunks.
- `lapis reindex --all` embeds every chunk after the walk and reports `embedded`, `embedError`, `embedder` and `embedderNote`. A terminal sees `embedding N chunks` while it runs. A failing embedder is reported and the keyword index still lands.
- Hybrid search fuses BM25 and vectors. When Ollama is down, hung, or slower than `timeout_ms`, search drops the vector arm and answers from BM25. `search --embedder none` is BM25 only and never connects; `--embedder ollama|auto` overrides the config for one query.
- Stored vectors survive anything short of embedding with a different model or width. `lapis-lattice` no longer treats an open with no embedder as a change of space: the desktop graph view, a run with no `[embedder]` section, `provider = "none"`, `--embedder none`, a dead Ollama, or a new model that does not answer all leave the stored space and its vectors alone. Before this, any keyword-only open dropped the vector table. `auto` uses the space the index records and probes Ollama only during `reindex --all`, `doctor` or the TUI build.
- `lapis reindex --vectors` embeds only the chunks that have no vector yet, with no walk and nothing cleared, so an Ollama outage or an interrupted run resumes instead of re-embedding the vault. `doctor` names it when vectors are missing.
- A one-note reindex (every write) embeds only that note's chunks, so the first write after configuring an embedder does not embed the whole vault inline.
- `doctor` reports the embedder: model, width, endpoint, chunks embedded out of chunks indexed, and one query round trip. Missing vectors and a dead Ollama warn; neither fails. Embedded `vault info --json` health carries `chunks` and `vectors`.

### Graph

- The graph unfolds in view. The layout still settles off the UI thread, but it publishes positions about thirty times a second, so the board shows the vault taking shape instead of a loading card. Unlinked notes ride a belt that grows with the core, and the final separation pass slides in over about a third of a second. The camera keeps the graph framed as it grows until you pan or zoom; Fit hands it back. Panning and zooming work during the unfold.
- Big graphs draw again. Each hairline costs about 720 bytes of GPU instance data, and the renderer drops any frame past 256 MiB. A 20,000-note vault with thousands of long unresolved links needed 480 MiB, so the board froze on its loading card. Links are now clipped to the board before they are drawn, unresolved links are dashed only while a frame has room for the dashes (past 60,000 dash segments they are drawn solid), and a frame draws at most 120,000 segments.
- The desktop prints GPUI's warnings and errors to stderr. The dropped frames above were reported all along, to a `log` call nothing printed.
- Zoomed all the way out, hubs (discs of 4 px or more) and the hovered note keep their names.
- Hovering a hub with more than 40 links draws its fan solid and with less gold ink, instead of hundreds of converging dashes that shimmered.

### Hardening

- A note path that resolves through an in-vault symlink to a file outside the vault is refused with exit 3 (`symlink escapes the vault`). This covers `read`, `check-note`, edits, task toggles and the MCP read tools, which all resolve paths the same way. Symlinks that stay inside the vault still work.
- The `phantom-fractal` child is bounded. It is stopped after 5 s, its stdout is read up to 1 MiB and its stderr up to 64 KiB, and either limit is a typed error (shown as a Phantom row in the desktop), never a hang or an unbounded buffer. It sees only `PATH`, `HOME`, `XDG_DATA_HOME`, `TMPDIR`, `USER`, `LANG`, `TZ`, `LC_*` and `PHANTOM_*` from Lapis's environment.
- `$LAPIS_PHANTOM_BINARY`, when set, is the only place Lapis looks for Phantom. A configured path that is missing, misnamed or not executable no longer falls back to `PATH`.
- Graph preview: a note path containing ` · ` shows whole on the preview card. Without the desktop services, the preview reads at most what it shows and stays inside the vault. Errors on the graph board no longer show where the vault lives on disk.

### Reindex everything

- `lapis reindex --all` rebuilds the whole embedded index in place: the same rebuild the TUI and the desktop run from their build-index actions, for a terminal or a script. A terminal sees `indexing N/M` on stderr while it runs; `--json` prints `documents`, `chunks`, `edges`, `embedded`, `embedError` and `elapsedMs`. Against an HTTP lattice it exits 1, because that service owns its own index. The stale-index hints in `search` and `doctor` name it.

### Stale index

- `search --json` carries `meta.stale`: how many notes the index has not caught up with. A note is stale when its file is newer than its index entry, when it sits in an indexed folder but has no entry, or when its entry names a file that is gone. Non-zero means a miss may be the index, not the vault. The MCP `search` tool reports it too, and human `search` output prints one line on stderr.
- `vault info` reports `lattice.stale` with `count`, `changed`, `unindexed`, `removed` and up to 20 paths. `doctor` adds an `index_fresh` check.
- The check reads the index through the configured backend, embedded or HTTP, and walks only folders that already hold indexed notes, so folders the index leaves out on purpose are never counted. Markdown only.
- Listing about 9,300 documents from an HTTP lattice takes most of a second, so `search` and `vault info` reuse a check up to 60 s old from `$XDG_CACHE_HOME/lapis` (else `~/.cache/lapis`), never from the vault. A reindex through `lapis` drops the cached check, and `doctor` always checks afresh.

### Desktop

- PDFs read as one continuous column of pages. Only pages near the viewport are rendered, one at a time, so a 1,300-page book opens and scrolls without rendering every page. The page field follows the scroll; typing a page, Previous/Next, j/k, Page Up/Down and Home/End jump to page tops, and zoom keeps your place.
- The window opens in a Lapis palette (night navy, cream, gold) unless an Omarchy theme is present. `LAPIS_THEME=lapis` or `LAPIS_THEME=omarchy` overrides the choice.
- The sidebar, toolbar, tabs, document views, search palette, command menu, context panel, PDF reader and status bar are restyled, with icons and shortcut hints.
- Live and Reading keep text in a centred column of about 72 characters. Live sets H1/H2 in a serif, colours `[[wikilinks]]`, dims HTML comments, shows strikethrough and inline code, and draws a scroll thumb.
- The sidebar file list scrolls. Before this, entries past the window height could not be reached.
- Arrow keys keep the palette and menu selection in view. A new PDF page opens at its top.
- The `Phantom not found` notice moved from a banner above every document to the status bar.
- Tabs with the same file name show their parent folder, not the full path. Opening a note scrolls its tab into view.
- Live shows `[[target|Label]]` as `Label`, and a click follows it: wikilinks resolve through the vault (exact path first, then the same rules as `lapis resolve`), Markdown links open in the browser. Alt-click places the caret instead. A link that names nothing, or more than one note, says so in the status bar.
- Frontmatter shows as a properties card above Live and Reading text: the first four keys (title, type, status, …), with `Show all` for the rest. Wikilinks in values, such as `parent` and `cross_refs`, follow like body links. The card is read-only; Properties in the command menu edits.
- Reading view follows wikilinks too. Search snippets read as one run of prose and clamp to two lines.
- The sidebar is a folder tree: folders open and close in place and load on first open, with indent guides and a collapse-all control. Opening a note opens the folders above it and scrolls its row into view. A Recent section lists the last five notes opened. The saved session format is unchanged.
- The search palette shows keyword results as soon as they arrive (a fraction of a second) and replaces them with the hybrid ranking when the embedder answers, keeping the selected note selected. If vector search fails, the keyword results stay and say so.

### Phantom client

- Lapis parses the JSON envelope that `phantom-fractal` prints and does not read Phantom's database.

### Notes and the RAG gate

- `lapis create` writes `title` beside `name`, so a new note passes the frontmatter gate it is checked against.
- The gate resolves wikilinks against the index the vault actually uses: the HTTP lattice when the config says `mode = "http"`, otherwise the embedded one. It used to read the embedded index regardless and report notes as dangling that `lapis resolve` found.
- A `/documents` row whose tags hold a number or a boolean (YAML reads `tags: [ops, 429]` that way) no longer fails the whole page. `lapis list` reported the lattice as down because of one such note.

### Dependencies

- `lapis-lattice` moved to rusqlite 0.40, and its behavior is unchanged.

### Graph

- The graph has the Lapis look: a lapis glow and a quiet starfield that drifts slightly with a pan, notes coloured by their top-level folder (six soft hues rotated from the theme accent, so the accent stays for the focused note and seeds), a faint halo on hubs, and hollow rings for unresolved links.
- Hovering or selecting a note gives it a gold ring and a label pill, draws its links in gold, and fades the rest of the board, labels included.
- The controls are a floating toolbar over the board (filter, layout, depth in local mode, orphans, unresolved, domain, colour group; fit, unpin, refresh, reset). A legend lists folder colours, colour groups and the counts, and a preview card shows the hovered note's title, path and opening lines. Loading, empty and failed boards say so in a card in the middle.

### In-place edit

- `lapis replace PATH --old TEXT --new TEXT` and `lapis insert PATH (--after|--before) ANCHOR (--text|--stdin)` edit one note by an exact raw-byte match. Zero or many matches, including overlaps, exit 1, leave the file unchanged, and report the hit count as JSON `error.matches`.
- `--json` errors and MCP error `data` carry `kind` and `exit`: `not_found` and `sandbox` (both exit 3), `stale_guard`, `match_count` (also `matches`), and `bad_args` (exit 1). The hash stays FNV-1a 64.
- `--if-hash`, `--if-mtime`, and `--dry-run` match `append`. The hash is FNV-1a 64 over the exact on-disk bytes (`hashAlgo: fnv1a64`, `oldHash`, `newHash`). A stale guard writes nothing. Dry-run returns a unified diff and does not bump `updated:` on disk, write, or reindex. `newHash` is that preview and excludes the bump. The dry-run still runs the frontmatter and wikilink gate on the bumped text, so it fails when the write would refuse.
- A real write bumps `updated:` and, when that key is missing, inserts it before the closing `---`. Every other frontmatter byte stays, including CRLF. A file with no frontmatter block does not gain one. The body is the only search region.
- The write is a same-directory temp file plus rename and keeps the file mode. `..`, an absolute path, and a symlink that leaves the vault exit 3 (`sandbox`). A missing note is exit 3 (`not_found`).
- MCP adds one tool, `lapis_edit`. `mode` is an enum: `replace` | `insert_after` | `insert_before`. The base tool count is 15.

### Desktop

- The optional code-graph sidecar was removed. There is no user-visible
  change unless that sidecar's config was present.

## 0.4.2 — 2026-09-21

The cut that ships `--rerank-jev`. Tag `v0.4.1` (2026-09-11) is already
published and does not include this flag. This section is the version and
notes for the next tag. The tag, GitHub Release, and `cargo publish` of
`lapis-lattice@0.4.2` happen after merge. Root `lapis` stays `publish = false`.

Crate `repository` and `homepage` are
`https://github.com/Hedronite/lapis-lattice`.

### Agent skill

- The vault skill is `.agents/skills/lapis/` (`SKILL.md` plus `lapis.nu`).
  The same bytes are regular files under `.claude/skills/lapis/`,
  `.grok/skills/lapis/`, and `.cursor/skills/lapis/`. `scripts/sync-skills`
  rewrites the copies. There is no root `skills/` tree.
- The skill maps 15 of the 17 MCP tools. The default server exposes 15;
  `tome_tree` and `tome_open` register only with `--features tome` and
  `LAPIS_TOME=1`. `health` and `search_and_read` have no CLI command.
  `lapis_edit` is the `replace` and `insert` wrappers.
- The skill wraps the existing CLI for `analytics`, `tree-retrieve`,
  `create`, `append`, `replace`, `insert`, `task list`, `task toggle`,
  `tome tree`, and `tome open` (`--features tome`). Search rows keep
  `path`, `title`, `score`, and `snippet`. A failing command exits with
  the envelope's `exit` code. Flags go before `--`. Text that starts
  with `-` follows `--`. Text that starts with `--` uses `--flag=value`
  or `--stdin`. A flag after `--` is a usage error and writes nothing.
  `tome build` is wrapped. A CLI error's `kind` and `matches` are copied
  onto the skill envelope unchanged.
- `append`, `task toggle`, `tome build`, `replace`, and `insert` reject
  a flag after a positional or after `--`. `append X.md hello --vault V`
  is a usage error and writes nothing. `read`, `search`, and `create`
  still accept a flag after their positionals. `replace` and `insert`
  forward `--dry-run`, `--if-mtime`, `--if-hash`, `--no-reindex`, and
  `--stdin`. They are the agent edit path. `create`, `append`, and
  `task toggle` run only when a human has asked.

### Jev rerank

- `lapis search … --rerank-jev` (MCP `search.rerank_jev`): post-retrieve
  Jev Noul/Score/Choice over top-k hits via the Facet TypeSafe recipe.
  Shadow only; empty/uncertain ≠ approve. Key from Facet env /
  `$TYPESAFE_API_KEY`, never Lattice. See [docs/jev-native.md](docs/jev-native.md).

### Search

- FTS matches path and title as well as body. Dotted names such as
  `AGENTS.md` stay searchable tokens. Identifier hits are boosted after
  retrieve.
- CLI, MCP, and TUI search share one `Hit` / `SearchParams` and the same
  operations layer. Hop-2 and `tree-retrieve` on the embedded index still
  refuse with a typed HTTP-only error instead of an empty result.

### TUI

- Pointer selection, clipboard copy, literal bracketed paste, and raw
  LF/CRLF paste. Undo is one transaction across those edits.
- YAML saves as the bytes you typed. PDF and HTML open as read-only
  references (no script, no fetch).
- An external change during save keeps the buffer; `Space l S` writes a
  copy beside it. A clean file reloads; a dirty one does not.
- Back/forward through notes, and search failure is its own state.
- A vault with no index gets an explicit background build, with files/total
  on the status line. Stale index drift is reported. Idle frames are not
  redrawn unless workspace state changed.

### Desktop

Built with `--features desktop` (off in the default CLI asset).

- Notes workspace: sidebar, find, Live Markdown with the caret block left
  as source, Vim motions, literal paste, resizable panes, and back/forward.
- Normal exit restores visible tabs, cursor, and pane layout. Split right
  and split below are independent documents; the same file is not mounted
  twice.
- Notes workflows (new note, daily/weekly/monthly, templates, tasks, trash)
  and a refresh when something else writes the vault.
- The vault graph opens inside the workspace on request. PDF pages render
  in a cancellable reader; the PDFium runtime is packaged beside the app
  and is never downloaded on open.
- An idle Live note no longer repaints on caret blink.

## 0.4.1 — 2026-09-11

Tagged `v0.4.1`. crates.io truth cut.

Git tag, Cargo (`lapis` and `lapis-lattice`), and `lapis --version` are
**0.4.1**. `lapis-lattice@0.4.1` is on crates.io; `0.1.0` stays as history.
The `v0.4.0` GitHub Release binary still carries crate `0.1.0` and is not
rewritten.

- Root `lapis` crate: `publish = false` (name squatted on crates.io); only `lapis-lattice` is publishable.
- Stranger README/install distinguished the Release binary from `cargo add lapis-lattice`.
- Operator-topology scrub: private build-host references removed from the public tree; `scrub-gate.sh` enforces.
- `pack-release` fails when root `lapis` crate version ≠ git tag.

Not in this tag: `feat/struct` (PR #15), `feat/api-v1`, and `--rerank-jev`.
Path/title search, identifier boost, the desktop notes workspace, and Jev
rerank are 0.4.2.

## 0.4.0 — 2026-09-11

Tagged `v0.4.0`. Operator GO 2026-09-11.

The vector arm becomes a SQLite query instead of a Rust table scan.

### sqlite-vec, linked not loaded

- The crate depends on `sqlite-vec` and registers it with
  `sqlite3_auto_extension` before the first connection opens, so every
  connection has `vec0`. There is no shared object to ship or find on `PATH`,
  and `load_extension` is never enabled, so a vault database cannot talk the
  process into loading code. A test calls `vec_version()` and asserts
  `load_extension` still refuses.
- Pinned to `0.1.9`, the newest release that builds. `0.1.10-alpha.4` does not:
  its `sqlite-vec.c` includes a `sqlite-vec-diskann.c` the package omits. That
  alpha also carries DiskANN, which this tag rules out, so the pin is deliberate
  rather than incidental.

### Embeddings live in `vec0`

- Chunk vectors moved from `embeddings(chunk_id, vec BLOB)` to a `vec0` virtual
  table beside `chunks_fts`, in the same database. One file is still the whole
  index and WAL is unchanged.
- The column is as wide as the embedder and uses `distance_metric=cosine`,
  because cosine is what the Rust scan computed. Taking `vec0`'s default L2
  would have quietly re-ranked every vault.
- The table is created only once a width is known, so a database opened with
  `--embedder none` has no vector table rather than an empty one.
- A pre-v0.4 blob table is adopted on open: rows that decode to exactly the
  right width are carried into `vec0` and the old table is retired, so upgrading
  does not cost a model run per chunk. Rows of another width belong to another
  embedding space and are dropped, which is the rule everywhere else.
- `vec0` carries no foreign key, so vectors are now cleared on a full reindex
  and removed per path on `reindex_path`. Without that, KNN would keep returning
  chunk ids that no longer name a chunk.

### `vector_rank` is KNN in SQL

- One `MATCH` with a `k`, ordered by distance, run inside SQLite. The corpus no
  longer crosses the FFI boundary on every query.
- Brute force on purpose. A test asserts the query names no approximate index:
  no DiskANN, HNSW, IVF or `INDEXED BY`.
- Fusion is unchanged: the same reciprocal rank fusion with FTS5 BM25, and
  hybrid search still reports `modalities`.
- Unchanged law: `--embedder none` is the default, a dead embedder omits the
  vector arm, and a dummy vector is never written. The named test that pins that
  behaviour is still green, now counting rows in `vec0`.
- The Rust `cosine` helper is gone. Leaving a second definition of "similar"
  next to the one SQLite now computes is how two answers to the same question
  start to disagree.

### Help text that named the wrong engine

- The CLI and the MCP tool description called the named analytics "DuckDB". On
  the embedded default they run on the same SQLite index as everything else.
  Corrected in both places.

## 0.3.1 — 2026-09-11

Tagged `v0.3.1`.

### Stranger-install copy (on `main` before this tag)

- README / `docs/install.md` match the tagged Release assets (no longer say Releases do not exist).
- macOS browser-download Gatekeeper gap is documented (`xattr` + ad-hoc `codesign`); `install.sh` clears quarantine when it can.
- `lapis --help` describes the embedded default instead of implying HTTP `:8080`.
- HTTP-down errors no longer mention `serve.py` / `launchctl kickstart`.

### `lapis init` configures the vault it creates

Creating a vault used to leave nothing behind that pointed at it. The resolver
reads `--vault`, then `$LAPIS_VAULT`, then `vault` in the config, and `init`
wrote none of the three, so `lapis`, `lapis tui` and `lapis desktop` still said
`no vault configured` after an init that had built the index. The error then
offered `lapis init` as the remedy, which is the command that had just failed to
help, so following the message returned you to the message.

- `lapis init <path>` writes `vault = "<path>"` into `~/.config/lapis/config.toml`
  and says which file it wrote. The next bare `lapis` resolves with no
  environment variable.
- Nothing is guessed: the operator named the path. `--vault` and `$LAPIS_VAULT`
  still win over the config.
- An existing `vault` key is never overwritten. A second `init` somewhere else
  reports that it left the configured vault alone.
- An existing config is edited, not rewritten: the key goes into the top-level
  table above the first section, so it cannot land inside one, and comments and
  ordering survive. A malformed config is refused rather than clobbered.
- The `no vault configured` message now names a remedy that leaves a vault
  configured.
- `scripts/install.sh` no longer tells you to export `LAPIS_VAULT` afterwards;
  the commands it prints work as typed.

### Docs that described something else

- `docs/install.md` documented a `lapis setup` command as "init + config + first
  index". There is no such subcommand; `init` is the whole of it, and the page
  now says so.
- The same page claimed `init` does not yet build `lattice.sqlite`. It does.

### CI

- The no-vault assertion runs against an empty `XDG_CONFIG_HOME`, so it tests
  the resolver rather than whatever an earlier step left on the runner.
- A new step runs `init` and then a bare `lapis` with no flag and no
  environment, and asserts search answers. That is the path that was broken.

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
- Linux binary is compiled on GitHub Actions `ubuntu-latest`. A missing `libxcb` / `libwayland` / `libvulkan` `.so` on the target machine is host config, not a product fail.

### Not in this tag

- Windows.
- Turso / DuckDB as first-run.
- Dummy embeddings.
- Merging `main` or tagging without operator GO.
