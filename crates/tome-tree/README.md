# tome-tree

Page-spanned table-of-contents trees for long PDFs. A judge walks the tree and opens page-cited passages. No vector step for "which section".

This crate is the library behind `lapis tome` (`--features tome`, off by default). MCP tools `tome_tree` and `tome_open` should call the frozen methods below. There is no `tome_search` tool in the spike: an agent walks with `tree` + `open`, or calls `walk` from Rust (`lapis tome search` and a future eval bin share that one loop).

Clean-room note: [PROVENANCE.md](PROVENANCE.md).

## Frozen interface

```rust
use tome_tree::{Budget, DocId, DocMeta, FakeJudge, Judge, NodeId, OpenPassages, TomeIndex};

let index = TomeIndex::open(index_dir)?;          // `<vault>/.lapis/tomes`
let docs: Vec<DocMeta> = index.docs()?;
let nodes = index.tree(&doc, node, depth)?;       // node: Option<&NodeId>, depth: Option<u8>
let passages = index.passages(&doc, &node_ids)?;  // same read as `open`
let _via_trait = index.open(&doc, &node_ids)?;    // needs `OpenPassages` in scope
let walked = index.walk(&doc, query, &judge, Budget::default())?;
```

Rust cannot overload `open`. The constructor is the associated function `TomeIndex::open(&Path)`. The passage read is the [`OpenPassages`] method, so `index.open(&doc, &ids)` works once the trait is imported. `OpenPassages::open(&index, &doc, &ids)` does not need method resolution.

Every method returns `Result`. An empty `Vec` means "no children", never "the doc is missing" or "the PDF has no structure".

| Method | Role |
| --- | --- |
| `TomeIndex::open(index_dir)` | Open or create the JSON store directory. |
| `docs` | List stored `DocMeta`. Does not re-hash PDFs. |
| `tree(doc, node, depth)` | Roots, or the one node named by `node`. `depth: None` is the full tree. `Some(0)` keeps the node and clears `children`. `child_count` stays the full count. |
| `open(doc, nodes)` / `passages(doc, nodes)` | One [`Passage`] per physical page. Hard cap **12 pages / 48 KB**. Over the cap is `over_budget`, not a clipped result. `truncated` is always `false`. A page missing from the store is an error. |
| `walk(doc, query, judge, budget)` | Judge the roots in batches (`root_batch_size`, default **16**), under a separate root budget (`root_calls`, default **4**). Keep beam **2**, then descend the same way. Descent spends `max_judge_calls` (default **24**) and batches sibling sets too. Stop at a leaf or a node of **≤ 3 pages**. Open whole nodes that fit the page and byte caps (highest score first). Nodes that do not fit are listed on `Walk.skipped` and are not clipped. Each scored candidate is on `Walk.judged` (`score`, `confidence`, `rank`). `rank` equals `score`. `Walk.root_path` is `batch` or `lexical_fallback`. |
| `build(pdf, vault_path, opts)` | Not part of the read surface. Writes the store. |
| `meta(doc)` | One `DocMeta`. Same staleness check as `tree`. |
| `content_id(path)` | SHA-256 doc id of a PDF. |

`Budget` defaults to 24 descent calls, 4 root calls, batches of 16, a lexical top-6, and 12 opened pages. The 48 KB cap always applies on `open` and `walk`. `DESCENT_RESERVE` (8) is still exported; frontiers are batched instead of truncated to it.

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
Walk { doc_id, query, nodes, passages, judge_calls, skipped, judged, root_judge_calls, root_path }
Judged { node_id, title, page_start, page_end, score, confidence, rank }
```

`level` is the depth in the tree. Roots are 1.

v0 `summary` is the `lead`: the first 400 characters of the node's page text. The summary model is recorded from config and is not called.

`source` on a node is how that node's bounds were chosen. A parent from the PDF outline can have `window` children when it was split. `DocMeta.source` is the source of the tree as a whole. `DocMeta.outline` is true when an outline survived front-matter cleanup.

## Build rules

1. PDF outline (`lopdf`), destinations resolved to physical pages. Titles are trimmed. `Cover`, `Contents`, `Table of Contents`, `Title Page`, and `TOC` are dropped. An item with no destination is skipped and its children are kept.
2. If there is no outline, heading detection: `CHAPTER n`, numbered `n.m Title`, and a short title-case line at the top of a page. Repeated running headers are suppressed.
3. `llm-struct` is a stub. It returns `no_structure` and does not invent a tree. The feature is off by default.
4. `--allow-windows` is the only way to build a page-window tree when both of the above fail. Those nodes have `source: "window"`.
5. A leaf longer than **10 pages**, **~20k tokens** (chars / 4), or **48 KB** of page text is split by inner headings, otherwise into page windows titled `{parent} (pp. a–b)`. A single page longer than 48 KB cannot be split further.
6. No outline and a failed fallback is `no_structure`. The store is not written. An empty tree is never returned.

## Store

```text
<vault>/.lapis/tomes/<sha256>.tree.json
<vault>/.lapis/tomes/<sha256>.pages.jsonl
```

`tree` / `open` / `walk` return `stale` when `builder_version` differs or the PDF at `DocMeta.path` exists and its bytes hash to something else. A missing file is not treated as a change: the doc id is the hash. After a hash matches, later calls skip the re-hash while that path's mtime and size stay the same. `lapis tome build --force` rebuilds.

`DocId::parse` / `TryFrom` accept 64 hex characters and reject anything else, so a doc id cannot escape the index directory. There is no `From<&str>`: an invalid id is an error, not a panic.

## Judge

```rust
pub trait Judge {
    fn score(&self, query: &str, candidate: &Candidate) -> Result<u8>;
    fn assess(&self, query: &str, candidate: &Candidate) -> Result<Assessment> { /* ... */ }
    fn score_batch(&self, query: &str, candidates: &[Candidate]) -> Result<Vec<Assessment>> { /* ... */ }
    fn batch_cost(&self, candidates: &[Candidate]) -> u32 { /* ... */ }
}
```

`score` stays the required method. `score_batch` returns one slot per candidate (`None` if that id was not scored) and defaults to one `assess` per candidate (`batch_cost` = that many calls). A batching judge reports `batch_cost` 1 for the batch request. Each `None` is another `assess`, and that call counts. A candidate's page `lead` and its `child_titles` are separate fields. The page lead is capped on its own (240 characters in a batch). Child titles are not recovered by parsing the lead.

The walk ranks on `score` only. `confidence` is recorded on `Walk.judged` and is not a gate: a low value does not abort the walk, drop the candidate, or change its rank. `judge_unavailable` is only a transport or model failure (no transport, a missing score, or a score outside 0..=3 on a one-at-a-time call).

A batched response that is malformed (wrong length, no parseable scores, or a score outside 0..=3) is not `judge_unavailable`. That failed call counts in `judge_calls`. The walk then pre-ranks the set by token overlap of the query with the title and lead, and judges the top `root_top_k` (default 6) one at a time. On the root pass those singles are a separate allowance, so `root_judge_calls` on that path is at most `1 + root_top_k` (the failed batch plus the singles) and is not limited by `root_calls`. A sibling set does the same inside the descent budget, and later sets skip the batch call once one has failed. `Walk.root_path` records which path the root pass ran. A batch that scored some ids and left others out keeps the scores it got. The walk judges only the missing ids, one at a time, until `root_calls` (or the descent budget) is spent. Every one of those calls is in `judge_calls`.

A page pdf-extract cannot safely read (a panic, a `/Parent` cycle, or a `Do` that is not a shallow Form) is extracted with lopdf for that page. One bad page does not fail the book. `/Kids` stored as an indirect array is still a page tree. One outline item whose destination does not resolve is skipped.

`FakeJudge` scripts scores by node id for offline tests. A missing id is `judge_unavailable`.

`lapis tome search` uses `jev::JevJudge`, which calls the existing transport (Facet, else `$TYPESAFE_API_KEY`, else none). One candidate reads the shipped relevance score (0–3). Several candidates are one call. The state asks for a JSON array of `{id, score, confidence}`, and the questions are one score rubric per candidate id, so the model is not locked to a single relevance score. The page lead and the child titles are written as separate fields. The parser accepts that array, or the per-id score answers System One returns. Ids that came back are kept. Missing ids are `None`, and the walk judges them one at a time. A body with neither shape is `parse`, and the walk takes the lexical fallback. The Facet batch body escapes `{` and `}` inside strings, so a `{{typesafeApiKey}}` in the PDF text stays literal.

```toml
[tome]
root_batch_size = 16
root_calls = 4
root_top_k = 6
```

The shipped System One questions still ask whether a *chunk* answers the query, and the recorded confidence is the minimum of the noul, relevance, and cite confidences. A TOC root is not a chunk. On a real book that showed up as relevance around 1.4 with confidence 0.29–0.41: the model was unsure, and the cite score pulled the minimum down. That confidence is kept on the walk and does not abort it. Eli's ruling matches the eval schema: rank on score only.

`JevJudge::score` blocks on the current tokio runtime with `block_in_place`. The `lapis` binary uses a multi-thread runtime. A current-thread runtime panics, and calling `walk` from inside an existing `block_on` can deadlock.

Jev does not write answers or summaries.

## Answer / summary model

Config, not a code constant. v0 still stores the lead as the summary; the model id is recorded on `DocMeta` so a later summary pass can read it.

```toml
[tome]
provider = "opencode"
model = "deepseek-v4.1-flash"
temperature = 0
root_batch_size = 16
root_calls = 4
root_top_k = 6
```

Defaults when the keys are omitted: provider `opencode`, model `deepseek-v4.1-flash`, temperature `0`. Change the file to swap the model. No rebuild of the judge path is required for that swap; the walk never calls this model.

## Errors

`TomeError::code()` is one of:

`unknown_doc` · `unknown_node` · `stale` · `no_structure` · `parse` · `io` · `over_budget` · `judge_unavailable`

`lapis --json` puts that string on both `error.kind` and `error.code`.

## CLI

```text
lapis tome build <pdf>... [--force] [--allow-windows] [--llm-struct]
lapis tome tree <doc> [--node ID] [--depth N]
lapis tome search <doc> "<query>" [--max-judge-calls 24] [--max-pages 12]
lapis tome open <doc> <node>...
```

`<doc>` is a sha256 or a path. Global `--json` uses the `{ok, data, error, meta}` envelope. The subcommands exist in every build; without `--features tome` they exit with a usage error and do not touch the store.
