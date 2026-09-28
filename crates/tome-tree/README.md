# tome-tree

Page-spanned table-of-contents trees for long PDFs. A judge walks the tree and opens page-cited passages. No vector step for "which section".

This crate is the library behind `lapis tome` (`--features tome`, off by default). MCP tools `tome_tree` and `tome_open` should call the frozen methods below. There is no `tome_search` tool in the spike: an agent walks with `tree` + `open`, or calls `walk` from Rust (`lapis tome search` and a future eval bin share that one loop).

Clean-room note: [PROVENANCE.md](PROVENANCE.md).

## Frozen interface

```rust
use tome_tree::{Budget, DocId, FakeJudge, Judge, NodeId, OpenPassages, TomeIndex};

let index = TomeIndex::open(index_dir)?;          // `<vault>/.lapis/tomes`
let docs: Vec<DocMeta> = index.docs()?;
let nodes = index.tree(&doc, node, depth)?;       // node: Option<&NodeId>, depth: Option<u8>
let passages = index.open(&doc, &node_ids)?;      // needs `OpenPassages` in scope
let walked = index.walk(&doc, query, &judge, Budget::default())?;
```

Rust cannot overload `open`. The constructor is the associated function `TomeIndex::open(&Path)`. The passage read is the [`OpenPassages`] method, so `index.open(&doc, &ids)` works once the trait is imported. `OpenPassages::open(&index, &doc, &ids)` does not need method resolution.

Every method returns `Result`. An empty `Vec` means "no children", never "the doc is missing" or "the PDF has no structure".

| Method | Role |
| --- | --- |
| `TomeIndex::open(index_dir)` | Open or create the JSON store directory. |
| `docs` | List stored `DocMeta`. Does not re-hash PDFs. |
| `tree(doc, node, depth)` | Roots, or the one node named by `node`. `depth: None` is the full tree. `Some(0)` keeps the node and clears `children`. `child_count` stays the full count. |
| `open(doc, nodes)` | One [`Passage`] per physical page. Hard cap **12 pages / 48 KB**. Over the cap is `over_budget`, not a clipped result. `truncated` is always `false`. |
| `walk(doc, query, judge, budget)` | Beam **2**. At each level, score children 0–3, keep the top 2, stop at a leaf or a node of **≤ 3 pages**, then open. |
| `build(pdf, vault_path, opts)` | Not part of the read surface. Writes the store. |
| `meta(doc)` | One `DocMeta`. Same staleness check as `tree`. |
| `content_id(path)` | SHA-256 doc id of a PDF. |

`Budget` defaults to 24 judge calls and 12 opened pages. The 48 KB cap always applies on `open` and `walk`.

## Types

Pages are **1-based physical PDF pages**, inclusive.

`DocId` is the lowercase hex SHA-256 of the PDF bytes. `NodeId` is a dotted path of 1-based sibling indexes, four digits each: `0003.0002`. Ids are stable for a given `(sha256, builder_version)`.

```text
Node {
  id, title, level, page_start, page_end, lead, summary,
  source,          // "outline" | "heading" | "llm" | "window"
  child_count, children
}

DocMeta {
  doc_id, path, sha256, pages, outline, source,
  built_at, builder_version,
  summary_model,           // "{provider}/{model}" from config
  summary_temperature
}

Passage { node_id, page, text, truncated }
Walk { doc_id, query, nodes, passages, judge_calls }
```

`level` is the depth in the tree. Roots are 1.

v0 `summary` is the `lead`: the first 400 characters of the node's page text. The summary model is recorded from config and is not called.

`source` on a node is how that node's bounds were chosen. A parent from the PDF outline can have `window` children when it was split. `DocMeta.source` is the source of the tree as a whole. `DocMeta.outline` is true when an outline survived front-matter cleanup.

## Build rules

1. PDF outline (`lopdf`), destinations resolved to physical pages. Titles are trimmed. `Cover`, `Contents`, `Table of Contents`, `Title Page`, and `TOC` are dropped.
2. If there is no outline, heading detection: `CHAPTER n`, numbered `n.m Title`, and a short title-case line at the top of a page. Repeated running headers are suppressed.
3. `llm-struct` is a stub. It returns `no_structure` and does not invent a tree. The feature is off by default.
4. `--allow-windows` is the only way to build a page-window tree when both of the above fail. Those nodes have `source: "window"`.
5. A leaf longer than **10 pages** or **~20k tokens** (chars / 4) is split by inner headings, otherwise into page windows titled `{parent} (pp. a–b)`.
6. No outline and a failed fallback is `no_structure`. The store is not written. An empty tree is never returned.

## Store

```text
<vault>/.lapis/tomes/<sha256>.tree.json
<vault>/.lapis/tomes/<sha256>.pages.jsonl
```

`tree` / `open` / `walk` return `stale` when `builder_version` differs or the PDF at `DocMeta.path` exists and its bytes hash to something else. A missing file is not treated as a change: the doc id is the hash. `lapis tome build --force` rebuilds.

## Judge

```rust
pub trait Judge {
    fn score(&self, query: &str, candidate: &Candidate) -> Result<u8>;
}
```

`FakeJudge` scripts scores by node id for offline tests. A missing id is `judge_unavailable`.

`lapis tome search` uses `jev::JevJudge`, which calls the existing transport (Facet, else `$TYPESAFE_API_KEY`, else none) and reads the shipped relevance score (0–3). `Transport::None`, an error, or an uncertain / low-confidence answer is `judge_unavailable`. The walk does not guess.

Jev does not write answers or summaries.

## Answer / summary model

Config, not a code constant. v0 still stores the lead as the summary; the model id is recorded on `DocMeta` so a later summary pass can read it.

```toml
[tome]
provider = "opencode"
model = "deepseek-v4.1-flash"
temperature = 0
```

Defaults when the keys are omitted: provider `opencode`, model `deepseek-v4.1-flash`, temperature `0`. Change the file to swap the model. No rebuild of the judge path is required for that swap; the walk never calls this model.

## Errors

`TomeError::code()` is one of:

`unknown_doc` · `unknown_node` · `stale` · `no_structure` · `parse` · `over_budget` · `judge_unavailable`

`lapis --json` puts that string on both `error.kind` and `error.code`.

## CLI

```text
lapis tome build <pdf>... [--force] [--allow-windows] [--llm-struct]
lapis tome tree <doc> [--node ID] [--depth N]
lapis tome search <doc> "<query>" [--max-judge-calls 24] [--max-pages 12]
lapis tome open <doc> <node>...
```

`<doc>` is a sha256 or a path. Global `--json` uses the `{ok, data, error, meta}` envelope. The subcommands exist in every build; without `--features tome` they exit with a usage error and do not touch the store.
