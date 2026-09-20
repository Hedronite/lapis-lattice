# Jev native rerank

Post-retrieve **judge** over top-k lattice hits. BM25 / embed / identifier
fusion stay the index. Jev classifies query↔chunk; it does not write notes
or replace the index.

Transport is the Facet TypeSafe / System One recipe (suite SoT). Lapis does
not link a TypeSafe SDK and does not store `$TYPESAFE_API_KEY` in
`lattice.sqlite` or vault notes.

## CLI

```bash
lapis search welcome --rerank-jev
lapis --json search welcome --rerank-jev --embedder none
```

`--rerank-jev` runs after retrieve, over the returned page (the same top-k
`--limit` / `--offset` already selected).

## MCP

`search` accepts `rerank_jev: true`. Same objects as `--json`. TUI / desktop
palette search does **not** call the gate (CLI + MCP exception).

## Shadow

Always on for this spike. A Choice / Noul / Score is **not** an authorization
to drop, keep, or rewrite a hit.

| Outcome | Meaning |
| --- | --- |
| empty / missing / low confidence | Not an approval. Original retrieve order stands. `jev.status=uncertain`. |
| `cite=unrelated` / low `answers` | Informative. Hit is still returned. |
| confident scores on every hit | Display order may follow Jev; `rank` stays the retrieve rank. `approved` stays false. |
| no Facet / no `$TYPESAFE_API_KEY` | Gate does not run. Hits unchanged. `jev.status=unavailable`. No network. |

`approved` is always `false` while shadow is on.

## Transport

1. `facet` on `$PATH` → `facet request run` against the bundled collection
   (`docs/examples/typesafe/opencollection.yml`, selector `items/0/items/0`),
   `--environment typesafe --no-record`. Key from Facet env store.
2. Else `$TYPESAFE_API_KEY` → `POST https://api.typesafe.ai/v1/systemone`
   (same recipe body). Key is read from the environment only.
3. Else unavailable. Offline tests use a fake transport.

`LAPIS_JEV_TRANSPORT=none|facet|http` forces a backend. `LAPIS_JEV_ENDPOINT`
overrides the System One URL for the HTTP path.

## Recipe (query↔chunk)

One System One call per hit:

| Question | Primitive | Use |
| --- | --- | --- |
| `answers` | Noul | Does this chunk answer the query? |
| `relevance` | Score (0–3) | How relevant is the chunk? |
| `cite` | Choice `supports` / `contradicts` / `unrelated` | Citation stretch |

Chunk text is clipped (`max_bytes` 2000). Path / title / heading stay bound
in the `state` string.

## Out

Replacing the index · auto-writing vault notes · Stanley / Pi · a second
TYPESAFE key in Lattice or notes.
