# lapis-lattice

Embedded **SQLite + FTS5** index for a folder of Markdown files.

This is the in-process recall engine for [Lapis](https://github.com/VirtualMachinist/lapis-lattice): BM25 search, a `[[wikilink]]` graph, and allowlisted analytics. It writes `<vault>/.lapis/lattice.sqlite` and **never** rewrites user notes.

- No HTTP daemon required
- No Turso / libsql
- No DuckDB
- Optional sqlite-vec KNN for hybrid search when an embedder is configured (`--embedder none` is the default)

## Install

This crate is the engine library only — it does not ship the `lapis` CLI/TUI binary.
For the app, see the [repo README](https://github.com/VirtualMachinist/lapis-lattice#install).

Live on crates.io today: `0.1.0` (`cargo add lapis-lattice`). After `v0.4.1` is tagged and published:

```bash
cargo add lapis-lattice@0.4.1
```

```rust,no_run
use lapis_lattice::Engine;

let mut engine = Engine::open("/path/to/vault")?;
engine.reindex()?;
for hit in engine.search("welcome", 10)? {
    println!("{}  {}", hit.rank, hit.path);
}
```

## Analytics

Named queries only (`inventory`, `priority`, `tags`, `health`, `recent`, `hubs`, `density`, `degree`, `dangling`). Raw SQL is rejected.

## License

MIT
