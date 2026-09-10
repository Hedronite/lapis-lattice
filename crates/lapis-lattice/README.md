# lapis-lattice

Embedded **SQLite + FTS5** index for a folder of Markdown files.

This is the in-process recall engine for [Lapis](https://github.com/VirtualMachinist/lapis): BM25 search, a `[[wikilink]]` graph, and allowlisted analytics. It writes `<vault>/.lapis/lattice.sqlite` and **never** rewrites user notes.

- No HTTP daemon required
- No Turso / libsql
- No DuckDB
- Vector/ONNX embeddings are out of scope for 0.1 (search is FTS5; hybrid is later)

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
